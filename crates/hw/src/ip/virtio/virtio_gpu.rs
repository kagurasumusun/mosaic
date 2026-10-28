//! virtio-gpu — the 2D display engine (virtio-gpu 1.1 / the "virtio-gpu"
//! device spec, commands as QEMU's `hw/display/virtio-gpu.c` decodes).
//!
//! Two virtqueues: `controlq` (0) and `cursorq` (1). Config space:
//! `events_read` u32 | `events_clear` u32 | `num_scanouts` u32 |
//! `num_capsets` u32 — the header every virtio-gpu probe reads.
//!
//! The control queue speaks the command protocol: each request is an
//! 8-byte `{type u32, flags u32}` header plus a type-specific body, and
//! the device answers each with a `virtio_gpu_resp_*` structure whose
//! `type` is the command type ORed with 0x1000 (VIRTIO_GPU_FLAG_ERR
//! unset). Commands implemented:
//!
//! * `GET_DISPLAY_INFO` — one 1024×768 scanout, enabled;
//! * `RESOURCE_CREATE_2D` — a 2D resource of a pixel format and size;
//! * `RESOURCE_ATTACH_BACKING` — the guest RAM range backing it;
//! * `TRANSFER_TO_HOST_2D` — copy a box from the backing into the
//!   resource (this model holds scanout contents in the host-side
//!   framebuffer);
//! * `RESOURCE_FLUSH` — publish (the scanout's bytes land in the
//!   framebuffer the machine glue reads for the front end);
//! * `SET_SCANOUT` — bind a resource to a scanout;
//! * `RESOURCE_UNREF` — drop a resource.
//!
//! Cursor-queue commands are accepted and answered OK without visible
//! effect (the cursor plane is a later layer). Unsupported commands get
//! `VIRTIO_GPU_RESP_ERR_UNSPEC` — the documented behaviour a driver
//! handles gracefully.

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

/// Command types (virtio_gpu_ctrl_type).
#[allow(dead_code)] // the documented command set; the walk grows into it
mod cmd {
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;
    pub const GET_CAPSET_INFO: u32 = 0x0108;
    pub const GET_CAPSET: u32 = 0x0109;
    pub const CURSOR_UPDATE: u32 = 0x0300;
    pub const CURSOR_POS: u32 = 0x0301;
    pub const RESP_NODATA: u32 = 0x1100;
    pub const RESP_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_ERR_UNSPEC: u32 = 0x1200;
    pub const RESP_ERR_OUT_OF_MEMORY: u32 = 0x1204;
    pub const RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1205;
    pub const RESP_ERR_INVALID_PARAMETER: u32 = 0x1202;
}

/// The pixel formats a guest actually picks (DRM fourcc).
mod fourcc {
    pub const B8G8R8X8: u32 = u32::from_le_bytes(*b"XR24");
    pub const B8G8R8A8: u32 = u32::from_le_bytes(*b"AR24");
}

/// One 2D resource with its guest-RAM backing.
#[derive(Clone, Debug, Default)]
struct Resource2d {
    width: u32,
    height: u32,
    #[allow(dead_code)] // validated at create; kept for resource queries
    format: u32,
    /// Backing guest ranges (address, length) in attach order.
    backing: Vec<(u64, u32)>,
    /// Host-side contents, filled by TRANSFER_TO_HOST_2D.
    pixels: Vec<u8>,
}

/// The framebuffer one scanout shows — what the machine glue reads.
#[derive(Clone, Debug)]
pub struct Scanout {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub const DEFAULT_WIDTH: u32 = 1024;
pub const DEFAULT_HEIGHT: u32 = 768;
const BYTES_PER_PIXEL: usize = 4;

pub struct VirtioGpu {
    resources: std::collections::BTreeMap<u32, Resource2d>,
    /// The single scanout (multimonitor is a config field away).
    scanout_resource: Option<u32>,
    scanout: Scanout,
    events_read: u32,
    num_scanouts: u32,
    /// The resource whose backing-entry table the next read-only chain
    /// carries (the ATTACH_BACKING follow-up).
    pending_attach: Option<u32>,
}

impl VirtioGpu {
    pub fn new() -> Self {
        Self {
            resources: std::collections::BTreeMap::new(),
            scanout_resource: None,
            scanout: Scanout {
                width: DEFAULT_WIDTH,
                height: DEFAULT_HEIGHT,
                pixels: vec![0; DEFAULT_WIDTH as usize * DEFAULT_HEIGHT as usize * BYTES_PER_PIXEL],
            },
            events_read: 0,
            num_scanouts: 1,
            pending_attach: None,
        }
    }

    /// The scanout's current frame (the front end's render source).
    pub fn scanout(&self) -> &Scanout {
        &self.scanout
    }

    #[allow(dead_code)]
    fn read_u32(mem: &mut dyn GuestMemory, address: u64) -> Result<u32, ProcessError> {
        let mut bytes = [0u8; 4];
        mem.read_bytes(address, &mut bytes)
            .map_err(ProcessError::from)?;
        Ok(u32::from_le_bytes(bytes))
    }

    #[allow(dead_code)]
    fn write_u32(mem: &mut dyn GuestMemory, address: u64, value: u32) -> Result<(), ProcessError> {
        mem.write_bytes(address, &value.to_le_bytes())
            .map_err(ProcessError::from)
    }

    /// Answer one control-queue chain: read the command, act, write the
    /// response into the chain's writable tail.
    #[allow(clippy::too_many_lines)]
    fn handle_control(
        &mut self,
        mem: &mut dyn GuestMemory,
        head: u32,
        slot: &mut QueueSlot,
    ) -> Result<(), ProcessError> {
        // Collect the chain segments: the request comes from read-only
        // bytes; the response goes into the writable tail.
        let mut segments: Vec<(u64, u32, bool)> = Vec::new();
        {
            let vq = slot.view();
            vq.walk(mem, head, |addr, len, writable, _index| {
                segments.push((addr, len, writable));
                Ok(())
            })?;
        }
        let request: Vec<u8> = {
            let mut data = Vec::new();
            for (addr, len, writable) in &segments {
                if *writable {
                    continue;
                }
                let mut chunk = vec![0u8; *len as usize];
                mem.read_bytes(*addr, &mut chunk)?;
                data.extend_from_slice(&chunk);
            }
            data
        };
        if request.len() < 8 {
            self.respond(mem, &segments, cmd::RESP_ERR_UNSPEC)?;
            return Ok(());
        }
        let command = u32::from_le_bytes(request[0..4].try_into().unwrap());
        // The writable tail (the response buffer).
        let writable: Vec<(u64, u32, bool)> = segments.clone();

        match command {
            cmd::GET_DISPLAY_INFO => {
                // struct virtio_gpu_ctrl_hdr is 24 bytes (type, flags,
                // fence_id u64, ctx_id, ring_idx); then one pmode:
                // rect{x,y,w,h u32 ×4} + enabled u32 + flags u32.
                let mut response = vec![0u8; 24 + 24];
                response[0..4].copy_from_slice(&cmd::RESP_DISPLAY_INFO.to_le_bytes());
                let rect = 24;
                response[rect..rect + 4].copy_from_slice(&0u32.to_le_bytes()); // x
                response[rect + 4..rect + 8].copy_from_slice(&0u32.to_le_bytes()); // y
                response[rect + 8..rect + 12].copy_from_slice(&self.scanout.width.to_le_bytes());
                response[rect + 12..rect + 16].copy_from_slice(&self.scanout.height.to_le_bytes());
                response[rect + 16..rect + 20].copy_from_slice(&1u32.to_le_bytes()); // enabled
                self.write_response(mem, &writable, &response)?;
            }
            cmd::RESOURCE_CREATE_2D => {
                // struct virtio_gpu_resource_create_2d: hdr | resource_id
                // | format | width | height.
                if request.len() < 24 {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_PARAMETER)?;
                    return Ok(());
                }
                let resource_id = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let format = u32::from_le_bytes(request[12..16].try_into().unwrap());
                let width = u32::from_le_bytes(request[16..20].try_into().unwrap());
                let height = u32::from_le_bytes(request[20..24].try_into().unwrap());
                if format != fourcc::B8G8R8X8 && format != fourcc::B8G8R8A8 {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_PARAMETER)?;
                    return Ok(());
                }
                let size = width as usize * height as usize * BYTES_PER_PIXEL;
                if size > 64 * 1024 * 1024 {
                    self.respond(mem, &segments, cmd::RESP_ERR_OUT_OF_MEMORY)?;
                    return Ok(());
                }
                self.resources.insert(
                    resource_id,
                    Resource2d {
                        width,
                        height,
                        format,
                        backing: Vec::new(),
                        pixels: vec![0; size],
                    },
                );
                self.respond(mem, &segments, cmd::RESP_NODATA)?;
            }
            cmd::RESOURCE_ATTACH_BACKING => {
                // hdr | resource_id | nr_entries, then a scatter-gather
                // follow-up command carries the entries; this model
                // reads the mem_entries the guest posts with the same
                // request (QEMU's two-step). Entries land in a second
                // chain the guest sends immediately — for the common
                // single-chain drivers, read them from the tail here.
                if request.len() < 16 {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_PARAMETER)?;
                    return Ok(());
                }
                let resource_id = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let entries = u32::from_le_bytes(request[12..16].try_into().unwrap());
                // The entry table arrives in the next request
                // (RESOURCE_ATTACH_BACKING with nr_entries > 0 is
                // followed by a memEntries chain); record the count and
                // take the addresses when TRANSFER runs, by walking the
                // backing list the guest registered.
                if let Some(resource) = self.resources.get_mut(&resource_id) {
                    resource.backing.clear();
                    resource.backing.reserve(entries as usize);
                }
                self.pending_attach = Some(resource_id);
                self.respond(mem, &segments, cmd::RESP_NODATA)?;
            }
            cmd::TRANSFER_TO_HOST_2D => {
                // hdr | resource_id | x y w h (u32×4) | offset u64 | stride
                // u32 | layer.
                if request.len() < 48 {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_PARAMETER)?;
                    return Ok(());
                }
                let resource_id = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let x = u32::from_le_bytes(request[12..16].try_into().unwrap());
                let y = u32::from_le_bytes(request[16..20].try_into().unwrap());
                let w = u32::from_le_bytes(request[20..24].try_into().unwrap());
                let h = u32::from_le_bytes(request[24..28].try_into().unwrap());
                let offset = u64::from_le_bytes(request[28..36].try_into().unwrap());
                let stride = u32::from_le_bytes(request[36..40].try_into().unwrap());
                let Some(resource) = self.resources.get_mut(&resource_id) else {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_RESOURCE_ID)?;
                    return Ok(());
                };
                // The backing: the guest RAM range attached to this
                // resource. With no explicit table (the single-buffer
                // path), the transfer offset addresses the first
                // backing entry directly.
                let (base, span) = resource.backing.first().copied().unwrap_or((0, u32::MAX));
                let stride = if stride == 0 { w * 4 } else { stride };
                for row in 0..h {
                    let source = base + offset + u64::from(row * stride);
                    let target = (y + row) as usize * resource.width as usize * 4 + x as usize * 4;
                    let count = (w as usize * 4)
                        .min(resource.pixels.len().saturating_sub(target))
                        .min(span as usize);
                    if count == 0 {
                        break;
                    }
                    let mut line = vec![0u8; count];
                    mem.read_bytes(source, &mut line)?;
                    resource.pixels[target..target + count].copy_from_slice(&line);
                }
                self.respond(mem, &segments, cmd::RESP_NODATA)?;
            }
            cmd::RESOURCE_FLUSH | cmd::SET_SCANOUT => {
                // RESOURCE_FLUSH: hdr | resource_id | x y w h. SET_SCANOUT:
                // hdr | x y w h (scanout rect) | resource_id (at 24).
                let id_offset = if command == cmd::RESOURCE_FLUSH {
                    8
                } else {
                    24
                };
                if request.len() < id_offset + 4 {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_PARAMETER)?;
                    return Ok(());
                }
                let resource_id =
                    u32::from_le_bytes(request[id_offset..id_offset + 4].try_into().unwrap());
                if command == cmd::SET_SCANOUT {
                    self.scanout_resource = if resource_id == 0 {
                        None
                    } else {
                        Some(resource_id)
                    };
                }
                // Publish the resource into the scanout framebuffer.
                if let Some(resource) = self.resources.get(&resource_id) {
                    let width = resource.width.min(self.scanout.width);
                    let height = resource.height.min(self.scanout.height);
                    for row in 0..height {
                        let source = row as usize * resource.width as usize * 4;
                        let target = row as usize * self.scanout.width as usize * 4;
                        let count = width as usize * 4;
                        if source + count > resource.pixels.len()
                            || target + count > self.scanout.pixels.len()
                        {
                            break;
                        }
                        self.scanout.pixels[target..target + count]
                            .copy_from_slice(&resource.pixels[source..source + count]);
                    }
                }
                self.respond(mem, &segments, cmd::RESP_NODATA)?;
            }
            cmd::RESOURCE_UNREF => {
                if request.len() < 12 {
                    self.respond(mem, &segments, cmd::RESP_ERR_INVALID_PARAMETER)?;
                    return Ok(());
                }
                let resource_id = u32::from_le_bytes(request[8..12].try_into().unwrap());
                self.resources.remove(&resource_id);
                if self.scanout_resource == Some(resource_id) {
                    self.scanout_resource = None;
                }
                self.respond(mem, &segments, cmd::RESP_NODATA)?;
            }
            cmd::GET_CAPSET_INFO | cmd::GET_CAPSET => {
                // No capsets: an empty answer the driver accepts.
                self.respond(mem, &segments, cmd::RESP_ERR_UNSPEC)?;
            }
            _ => {
                self.respond(mem, &segments, cmd::RESP_ERR_UNSPEC)?;
            }
        }
        Ok(())
    }

    /// The backing-entry table the guest sends after ATTACH_BACKING:
    /// a read-only chain of 16-byte `virtio_gpu_mem_entry` records.
    fn handle_attach_entries(
        &mut self,
        mem: &mut dyn GuestMemory,
        head: u32,
        slot: &mut QueueSlot,
    ) -> Result<(), ProcessError> {
        let resource_id = self.pending_attach.take().unwrap_or(0);
        let mut data = Vec::new();
        {
            let vq = slot.view();
            vq.read_chain(mem, head, &mut data)?;
        }
        if let Some(resource) = self.resources.get_mut(&resource_id) {
            for entry in data.chunks_exact(16) {
                let addr = u64::from_le_bytes(entry[0..8].try_into().unwrap());
                let len = u32::from_le_bytes(entry[8..12].try_into().unwrap());
                resource.backing.push((addr, len));
            }
        }
        Ok(())
    }

    /// Whether one chain carries only read-only descriptors (the
    /// attach-entries follow-up shape).
    fn chain_is_read_only(
        &self,
        mem: &mut dyn GuestMemory,
        slot: &mut QueueSlot,
        head: u32,
    ) -> Result<bool, ProcessError> {
        let mut read_only = true;
        let vq = slot.view();
        vq.walk(mem, head, |_addr, _len, writable, _index| {
            if writable {
                read_only = false;
            }
            Ok(())
        })?;
        Ok(read_only)
    }

    fn respond(
        &mut self,
        mem: &mut dyn GuestMemory,
        segments: &[(u64, u32, bool)],
        response_type: u32,
    ) -> Result<(), ProcessError> {
        let body = [0u8; 8];
        let mut response = response_type.to_le_bytes().to_vec();
        response.extend_from_slice(&body[4..]);
        self.write_response(mem, segments, &response)
    }

    fn write_response(
        &mut self,
        mem: &mut dyn GuestMemory,
        segments: &[(u64, u32, bool)],
        response: &[u8],
    ) -> Result<(), ProcessError> {
        let mut offset = 0usize;
        for (addr, len, writable) in segments.iter().rev() {
            if !*writable || offset >= response.len() {
                continue;
            }
            let take = (*len as usize).min(response.len() - offset);
            let slice = &response[response.len() - offset - take..response.len() - offset];
            mem.write_bytes(*addr, slice).map_err(ProcessError::from)?;
            offset += take;
        }
        Ok(())
    }
}

impl Default for VirtioGpu {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioGpu {
    fn device_id(&self) -> u32 {
        device_id::GPU
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1 | (1 << 0) // VIRTIO_GPU_F_EDID-less single scanout
    }

    fn max_queue_size(&self) -> u32 {
        64
    }

    fn queue_count(&self) -> usize {
        2
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        let mut config = [0u8; 16];
        config[0..4].copy_from_slice(&self.events_read.to_le_bytes());
        config[8..12].copy_from_slice(&self.num_scanouts.to_le_bytes());
        config[12..16].copy_from_slice(&1u32.to_le_bytes()); // num_capsets
        let start = offset as usize;
        if start + size as usize > config.len() {
            return 0;
        }
        let mut value = 0u64;
        for (index, byte) in config[start..start + size as usize].iter().enumerate() {
            value |= u64::from(*byte) << (index * 8);
        }
        value
    }

    fn config_write(&mut self, offset: u64, _size: u8, value: u64) {
        if offset == 4 {
            // events_clear: the guest acks display events.
            self.events_read &= !(value as u32);
        }
    }

    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        match queue {
            0 => {
                while let Some(head) = slot.view().next_head(mem)? {
                    if self.pending_attach.is_some() && self.chain_is_read_only(mem, slot, head)? {
                        self.handle_attach_entries(mem, head, slot)?;
                    } else {
                        self.handle_control(mem, head, slot)?;
                    }
                    slot.view().complete(mem, head, 0)?;
                }
            }
            1 => {
                // Cursor queue: acknowledge without effect.
                while let Some(head) = slot.view().next_head(mem)? {
                    slot.view().complete(mem, head, 0)?;
                }
            }
            _ => {}
        }
        irq.queue_interrupt();
        Ok(())
    }

    fn reset(&mut self) {
        self.resources.clear();
        self.scanout_resource = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip::virtio::test_ctx::support::RecCtx;
    use crate::ip::virtio::vqueue::tests_support::TestRam;
    use crate::ip::virtio::vqueue::{QueueConfig, F_WRITE};

    /// A control chain: read-only command bytes at 0x4000 chained to the
    /// writable response buffer at 0x6000 (one virtqueue chain, as a
    /// driver posts it).
    fn control_rig(ram: &mut TestRam, command: &[u8]) -> QueueSlot {
        let base = 0x1000;
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&0x4000u64.to_le_bytes());
        // A fixed 64-byte read-only window: `submit` refreshes the bytes,
        // and the descriptor length must not shrink with each command.
        entry[8..12].copy_from_slice(&64u32.to_le_bytes());
        entry[12..14].copy_from_slice(&crate::ip::virtio::vqueue::F_NEXT.to_le_bytes());
        entry[14..16].copy_from_slice(&1u16.to_le_bytes());
        ram.bytes[0..16].copy_from_slice(&entry);
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&0x6000u64.to_le_bytes());
        entry[8..12].copy_from_slice(&64u32.to_le_bytes());
        entry[12..14].copy_from_slice(&F_WRITE.to_le_bytes());
        ram.bytes[16..32].copy_from_slice(&entry);
        ram.bytes[(0x4000 - base) as usize..][..command.len()].copy_from_slice(command);
        ram.bytes[(0x2002 - base) as usize..][..2].copy_from_slice(&1u16.to_le_bytes());
        ram.bytes[(0x2004 - base) as usize..][..2].copy_from_slice(&0u16.to_le_bytes());
        let mut slot = QueueSlot::new();
        slot.config = QueueConfig {
            size: 8,
            desc_addr: 0x1000,
            avail_addr: 0x2000,
            used_addr: 0x3000,
            ready: true,
        };
        slot
    }

    fn run(gpu: &mut VirtioGpu, slot: &mut QueueSlot, ram: &mut TestRam) {
        let mut status = 0u32;
        let mut irq = VirtioIrq::new(
            mosaic_exec::SignalId(0),
            mosaic_exec::NodeId(0),
            &mut status,
        );
        let mut ctx = RecCtx::new();
        VirtioDevice::process(gpu, &mut ctx, 0, slot, ram, &mut irq).unwrap();
    }

    fn response_type(ram: &TestRam) -> u32 {
        u32::from_le_bytes(
            ram.bytes[(0x6000 - 0x1000) as usize..][..4]
                .try_into()
                .unwrap(),
        )
    }

    #[test]
    fn config_reports_one_scanout() {
        let gpu = VirtioGpu::new();
        assert_eq!(gpu.config_read(8, 4), 1);
        assert_eq!(gpu.config_read(12, 4), 1);
    }

    #[test]
    fn display_info_answers_the_scanout_geometry() {
        let mut gpu = VirtioGpu::new();
        let mut ram = TestRam::new(0x1_0000);
        let mut command = vec![0u8; 8];
        command[0..4].copy_from_slice(&cmd::GET_DISPLAY_INFO.to_le_bytes());
        let mut slot = control_rig(&mut ram, &command);
        run(&mut gpu, &mut slot, &mut ram);
        assert_eq!(response_type(&ram), cmd::RESP_DISPLAY_INFO);
        let width = u32::from_le_bytes(
            ram.bytes[(0x6000 - 0x1000 + 24 + 8) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(width, DEFAULT_WIDTH);
    }

    #[test]
    fn create_transfer_flush_round_trip() {
        let mut gpu = VirtioGpu::new();
        let mut ram = TestRam::new(0x2_0000);
        let base = 0x1000;
        // One slot for the whole conversation; ring entries name the head
        // descriptor of each request (0 = command chain, 2 = entry chain).
        let mut slot = control_rig(&mut ram, &[]);
        // Descriptor 2: the read-only entry chain at 0x7000 (16 bytes,
        // no next).
        let mut entry_desc = [0u8; 16];
        entry_desc[0..8].copy_from_slice(&0x7000u64.to_le_bytes());
        entry_desc[8..12].copy_from_slice(&16u32.to_le_bytes());
        ram.bytes[32..48].copy_from_slice(&entry_desc);

        let mut avail = 0u16;
        let mut submit = |ram: &mut TestRam, command: &[u8], head: u16| {
            ram.bytes[(0x4000 - base) as usize..][..command.len()].copy_from_slice(command);
            avail += 1;
            ram.bytes[(0x2002 - base) as usize..][..2].copy_from_slice(&avail.to_le_bytes());
            let ring = 0x2004 + 2 * u64::from(avail - 1);
            ram.bytes[(ring - base) as usize..][..2].copy_from_slice(&head.to_le_bytes());
        };
        let run_all = |gpu: &mut VirtioGpu, slot: &mut QueueSlot, ram: &mut TestRam| {
            let mut status = 0u32;
            let mut irq = VirtioIrq::new(
                mosaic_exec::SignalId(0),
                mosaic_exec::NodeId(0),
                &mut status,
            );
            let mut ctx = RecCtx::new();
            VirtioDevice::process(gpu, &mut ctx, 0, slot, ram, &mut irq).unwrap();
        };

        // 1. RESOURCE_CREATE_2D: 4x2 pixels, resource id 7.
        let mut command = vec![0u8; 24];
        command[0..4].copy_from_slice(&cmd::RESOURCE_CREATE_2D.to_le_bytes());
        command[8..12].copy_from_slice(&7u32.to_le_bytes());
        command[12..16].copy_from_slice(&fourcc::B8G8R8X8.to_le_bytes());
        command[16..20].copy_from_slice(&4u32.to_le_bytes());
        command[20..24].copy_from_slice(&2u32.to_le_bytes());
        submit(&mut ram, &command, 0);
        run_all(&mut gpu, &mut slot, &mut ram);
        assert_eq!(response_type(&ram), cmd::RESP_NODATA);

        // 2. RESOURCE_ATTACH_BACKING: one entry follows.
        let mut command = vec![0u8; 16];
        command[0..4].copy_from_slice(&cmd::RESOURCE_ATTACH_BACKING.to_le_bytes());
        command[8..12].copy_from_slice(&7u32.to_le_bytes());
        command[12..16].copy_from_slice(&1u32.to_le_bytes());
        submit(&mut ram, &command, 0);
        run_all(&mut gpu, &mut slot, &mut ram);

        // 3. The entry chain (read-only, at 0x7000 via descriptor 2):
        //    the entry table itself lives at 0x7000; the pattern it
        //    names lives at 0x5000.
        let pattern: Vec<u8> = (0..32u8).collect();
        ram.bytes[(0x5000 - base) as usize..][..32].copy_from_slice(&pattern);
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&0x5000u64.to_le_bytes());
        entry[8..12].copy_from_slice(&32u32.to_le_bytes());
        ram.bytes[(0x7000 - base) as usize..][..16].copy_from_slice(&entry);
        submit(&mut ram, &[], 2);
        run_all(&mut gpu, &mut slot, &mut ram);
        assert_eq!(gpu.resources[&7].backing, vec![(0x5000, 32)]);

        // 4. TRANSFER_TO_HOST_2D: box 4x2 at (0,0), stride 16.
        let mut command = vec![0u8; 48];
        command[0..4].copy_from_slice(&cmd::TRANSFER_TO_HOST_2D.to_le_bytes());
        command[8..12].copy_from_slice(&7u32.to_le_bytes());
        command[20..24].copy_from_slice(&4u32.to_le_bytes()); // w
        command[24..28].copy_from_slice(&2u32.to_le_bytes()); // h
        command[36..40].copy_from_slice(&16u32.to_le_bytes()); // stride
        submit(&mut ram, &command, 0);
        run_all(&mut gpu, &mut slot, &mut ram);
        assert_eq!(response_type(&ram), cmd::RESP_NODATA);

        // 5. FLUSH publishes into the scanout.
        let mut command = vec![0u8; 24];
        command[0..4].copy_from_slice(&cmd::RESOURCE_FLUSH.to_le_bytes());
        command[8..12].copy_from_slice(&7u32.to_le_bytes());
        submit(&mut ram, &command, 0);
        run_all(&mut gpu, &mut slot, &mut ram);
        assert_eq!(&gpu.scanout.pixels[..8], &pattern[..8]);
    }

    #[test]
    fn unsupported_command_answers_unspec() {
        let mut gpu = VirtioGpu::new();
        let mut ram = TestRam::new(0x1_0000);
        let mut command = vec![0u8; 8];
        command[0..4].copy_from_slice(&0xDEAD_0000u32.to_le_bytes());
        let mut slot = control_rig(&mut ram, &command);
        run(&mut gpu, &mut slot, &mut ram);
        assert_eq!(response_type(&ram), cmd::RESP_ERR_UNSPEC);
    }
}
