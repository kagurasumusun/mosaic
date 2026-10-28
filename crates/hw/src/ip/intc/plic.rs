//! RISC-V Platform-Level Interrupt Controller (PLIC) — SiFive-compatible
//! register map (the layout QEMU's `hw/intc/sifive_plic.c` and every
//! virt-class board implements).
//!
//! Clean-room from the SiFive PLIC spec (Interrupts Local Interrupt
//! Controller chapter) and the RISC-V PLIC chapter of the privileged
//! architecture companion. The register map, one context (hart + mode):
//!
//! ```text
//! 0x000000  reserved
//! 0x000004  source 1 priority     (word per source; 0 = never interrupt)
//! 0x001000  context 0 enable      (bit per source, word-strided)
//! 0x200000  context 0 threshold
//! 0x200004  context 0 claim/complete
//! ```
//!
//! Semantics implemented exactly as the spec's two rules:
//!
//! * an interrupt **signals** (the hart's `meip`/`seip` line goes high)
//!   when it is enabled, its priority is non-zero and strictly greater
//!   than the context threshold, and no lower-numbered source of the same
//!   or higher priority is claiming attention (the "maximum priority"
//!   selection of the spec — ties break to the lowest source ID, which is
//!   what hardware arbitration networks do and what `claim` must return
//!   for the same configuration);
//! * **claim** returns the signalled source with the highest priority
//!   (ties to the lowest ID) and puts it in the *in-progress* state — the
//!   line drops until **complete** (a write of the same ID) retires it.
//!   A complete write of a different ID is ignored (the spec makes it a
//!   no-op, not an error).
//!
//! Gate-level detail that matters for real drivers: the enable array and
//! priority array cover sources `1..=sources`; source 0 is reserved and
//! reads priority 0. `claim` of an idle context returns 0. The threshold
//! register holds the value sources must strictly exceed.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

/// Default source count (virt-class machines expose 54 or so; 96 is the
/// next power-of-two-ish window that keeps the enable block one page).
pub const DEFAULT_SOURCES: u32 = 96;
/// Default context (hart+mode) count.
pub const DEFAULT_CONTEXTS: u32 = 1;

/// Offsets, as words from the base.
pub const PRIORITY_BASE: u64 = 0x0000;
pub const PENDING_BASE: u64 = 0x1000;
/// Enable-array base (per-context enable words follow contiguously).
pub const ENABLE_BASE: u64 = 0x2000;
/// Hart context (threshold + claim) base.
pub const CONTEXT_BASE: u64 = 0x200_000;
/// Per-context stride in the hart context block (SiFive FU540 layout).
const CONTEXT_STRIDE: u64 = 0x1000;

#[derive(Clone, Copy, Debug)]
pub struct PlicParams {
    /// Number of interrupt sources (IDs 1..=sources).
    pub sources: u32,
    /// Number of contexts (hart × privilege mode slots).
    pub contexts: u32,
}

impl Default for PlicParams {
    fn default() -> Self {
        Self {
            sources: DEFAULT_SOURCES,
            contexts: DEFAULT_CONTEXTS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlicState {
    pub schema: u32,
    /// Priority per source (index 0 unused, IDs 1..=sources).
    pub priority: Vec<u32>,
    /// Level state of the inputs (bit i = source i+1... stored per source
    /// ID: bit (id-1)).
    pub level: Vec<u32>,
    /// Enable words per context.
    pub enable: Vec<Vec<u32>>,
    /// In-progress (claimed, not yet completed) bits per context.
    pub in_progress: Vec<u32>,
    /// Threshold per context.
    pub threshold: Vec<u32>,
}

pub struct Plic {
    state: PlicState,
    params: PlicParams,
    /// The external-interrupt output per context (machine glue wires the
    /// hart's `meip` input).
    outputs: Vec<Option<SignalId>>,
    /// Source-ID bindings: `(source node, source signal) → PLIC source
    /// ID`. Boards that wire device IRQ outputs into the PLIC through
    /// `inputs:<source_id>` register each wire here; `on_signal` looks
    /// the source ID up. Without a binding, `on_signal` falls back to
    /// `signal.0` as the source ID — the legacy convention a few unit
    /// tests still use.
    source_inputs: std::collections::HashMap<(mosaic_exec::NodeId, SignalId), u32>,
}

impl Plic {
    pub fn new(params: PlicParams) -> Self {
        let sources = params.sources.max(1);
        let contexts = params.contexts.max(1);
        let params = PlicParams { sources, contexts };
        let words = (sources as usize).div_ceil(32);
        Self {
            state: PlicState {
                schema: 1,
                priority: vec![0; sources as usize + 1],
                level: vec![0; words],
                enable: vec![vec![0; words]; contexts as usize],
                in_progress: vec![0; contexts as usize],
                threshold: vec![0; contexts as usize],
            },
            params,
            outputs: vec![None; contexts as usize],
            source_inputs: std::collections::HashMap::new(),
        }
    }

    /// Wire a context's external-interrupt output.
    pub fn bind_output(&mut self, context: usize, signal: SignalId) {
        if let Some(slot) = self.outputs.get_mut(context) {
            *slot = Some(signal);
        }
    }

    /// Bind one device's IRQ output to PLIC source `source_id`. The
    /// catalog spawn iterates `ctx.incoming` and registers each
    /// `inputs:<source_id>` wire here; `on_signal` then maps the
    /// (source, signal) pair back to the source ID. Without this binding,
    /// `on_signal` falls back to `signal.0` as the source ID (the legacy
    /// convention).
    pub fn bind_source_input(
        &mut self,
        source_id: u32,
        source: mosaic_exec::NodeId,
        signal: SignalId,
    ) {
        self.source_inputs.insert((source, signal), source_id);
    }

    fn words(&self) -> usize {
        (self.params.sources as usize).div_ceil(32)
    }

    /// Drive a source's input level (fabric-facing input; the machine
    /// routes a device IRQ output here by source ID).
    pub fn set_source_level(&mut self, ctx: &mut dyn Ctx, source: u32, high: bool) {
        if source == 0 || source > self.params.sources {
            return;
        }
        let word = ((source - 1) / 32) as usize;
        let bit = 1u32 << ((source - 1) % 32);
        if high {
            self.state.level[word] |= bit;
        } else {
            self.state.level[word] &= !bit;
        }
        self.update_outputs(ctx);
    }

    /// The highest-priority signalled source of `context`: enabled,
    /// pending, not in-progress, priority > threshold. Ties break to the
    /// lowest source ID (the arbitration rule the claim register must
    /// reproduce).
    fn best_signal(&self, context: usize) -> Option<u32> {
        let threshold = self.state.threshold.get(context).copied().unwrap_or(0);
        let enabled = self.state.enable.get(context)?;
        let in_progress = self.state.in_progress.get(context).copied().unwrap_or(0);
        let mut best: Option<(u32, u32)> = None; // (priority, source)
        for source in 1..=self.params.sources {
            let bit = 1u32 << ((source - 1) % 32);
            let word = ((source - 1) / 32) as usize;
            if self.state.level.get(word).copied().unwrap_or(0) & bit == 0 {
                continue;
            }
            if enabled.get(word).copied().unwrap_or(0) & bit == 0 {
                continue;
            }
            if in_progress & bit != 0 {
                continue;
            }
            let priority = self
                .state
                .priority
                .get(source as usize)
                .copied()
                .unwrap_or(0);
            if priority == 0 || priority <= threshold {
                continue;
            }
            if best.is_none_or(|(bp, _)| priority > bp) {
                best = Some((priority, source));
            }
        }
        best.map(|(_, source)| source)
    }

    fn update_outputs(&self, ctx: &mut dyn Ctx) {
        for context in 0..self.params.contexts as usize {
            if let Some(signal) = self.outputs.get(context).copied().flatten() {
                let high = self.best_signal(context).is_some();
                let _ = ctx.set_signal(signal, high);
            }
        }
    }

    /// The context claim: returns the best signalled source and marks it
    /// in-progress.
    fn claim(&mut self, ctx: &mut dyn Ctx, context: usize) -> u32 {
        let source = self.best_signal(context).unwrap_or(0);
        if source != 0 {
            let bit = 1u32 << ((source - 1) % 32);
            if let Some(word) = self.state.in_progress.get_mut(context) {
                *word |= bit;
            }
            self.update_outputs(ctx);
        }
        source
    }

    /// The context complete: retires an in-progress source.
    fn complete(&mut self, ctx: &mut dyn Ctx, context: usize, source: u32) {
        if source == 0 || source > self.params.sources {
            return;
        }
        let bit = 1u32 << ((source - 1) % 32);
        if let Some(word) = self.state.in_progress.get_mut(context) {
            *word &= !bit;
        }
        self.update_outputs(ctx);
    }
}

impl Default for Plic {
    fn default() -> Self {
        Self::new(PlicParams::default())
    }
}

impl MmioTarget for Plic {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let words = self.words() as u64;
        // Priority: word per source ID (1..=sources).
        if (4..4 + 4 * self.params.sources as u64).contains(&addr) && addr % 4 == 0 {
            let source = ((addr - 4) / 4) as usize;
            return Ok(u64::from(
                self.state.priority.get(source).copied().unwrap_or(0),
            ));
        }
        // Pending: read-only snapshot of the input levels.
        if (PENDING_BASE..PENDING_BASE + 4 * words).contains(&addr) && addr % 4 == 0 {
            let word = ((addr - PENDING_BASE) / 4) as usize;
            return Ok(u64::from(self.state.level.get(word).copied().unwrap_or(0)));
        }
        // Enable: contiguous per-context arrays of `words` words.
        if (ENABLE_BASE..CONTEXT_BASE).contains(&addr) {
            let stride = 4 * words;
            let rel = addr - ENABLE_BASE;
            let context = (rel / stride) as usize;
            let word = ((rel % stride) / 4) as usize;
            if rel % 4 == 0 {
                return Ok(u64::from(
                    self.state
                        .enable
                        .get(context)
                        .and_then(|banks| banks.get(word).copied())
                        .unwrap_or(0),
                ));
            }
            return Ok(0);
        }
        // Hart context: threshold at +0, claim/complete at +4, stride
        // 0x1000 per context (SiFive FU540 memory map).
        if addr >= CONTEXT_BASE && addr % 4 == 0 {
            let rel = addr - CONTEXT_BASE;
            let context = (rel / CONTEXT_STRIDE) as usize;
            let within = rel % CONTEXT_STRIDE;
            if within == 0 {
                return Ok(u64::from(
                    self.state.threshold.get(context).copied().unwrap_or(0),
                ));
            }
            if within == 4 {
                let value = self.claim(ctx, context);
                return Ok(u64::from(value));
            }
            return Ok(0);
        }
        Ok(0)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let words = self.words() as u64;
        if (4..4 + 4 * self.params.sources as u64).contains(&addr) && addr % 4 == 0 {
            let source = ((addr - 4) / 4) as usize;
            if let Some(slot) = self.state.priority.get_mut(source) {
                *slot = (value as u32) & 0x7;
            }
            self.update_outputs(ctx);
            return Ok(());
        }
        if (PENDING_BASE..PENDING_BASE + 4 * words).contains(&addr) {
            return Ok(()); // pending is read-only
        }
        if (ENABLE_BASE..CONTEXT_BASE).contains(&addr) {
            let stride = 4 * words;
            let rel = addr - ENABLE_BASE;
            let context = (rel / stride) as usize;
            let word = ((rel % stride) / 4) as usize;
            if rel % 4 == 0 {
                if let Some(banks) = self.state.enable.get_mut(context) {
                    if let Some(slot) = banks.get_mut(word) {
                        *slot = value as u32;
                    }
                }
                self.update_outputs(ctx);
            }
            return Ok(());
        }
        if addr >= CONTEXT_BASE && addr % 4 == 0 {
            let rel = addr - CONTEXT_BASE;
            let context = (rel / CONTEXT_STRIDE) as usize;
            let within = rel % CONTEXT_STRIDE;
            if within == 0 {
                if let Some(slot) = self.state.threshold.get_mut(context) {
                    *slot = (value as u32) & 0x7;
                }
                self.update_outputs(ctx);
                return Ok(());
            }
            if within == 4 {
                self.complete(ctx, context, value as u32);
                return Ok(());
            }
        }
        Ok(())
    }
}

impl EventTarget for Plic {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        ctx: &mut dyn Ctx,
        source: mosaic_exec::NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        // Source ID is looked up from the catalog-registered binding (the
        // `inputs:<source_id>` wiring); without a binding, the legacy
        // convention is `signal.0 == source_id` (used by self-tests that
        // drive the model directly).
        let source_id = self
            .source_inputs
            .get(&(source, signal))
            .copied()
            .unwrap_or(signal.0);
        self.set_source_level(ctx, source_id, high);
        Ok(())
    }
}

impl Snapshot for Plic {
    type State = PlicState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        self.state.clone()
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.clone();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_exec::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        for word in &state.level {
            hash.u32(*word);
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{AccessKind, Endian, NodeId, Security};

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0xFF,
            endian: Endian::Little,
        }
    }

    struct RecCtx {
        signals: Vec<(SignalId, bool)>,
    }
    impl RecCtx {
        fn new() -> Self {
            Self {
                signals: Vec::new(),
            }
        }
        fn level(&self, id: SignalId) -> Option<bool> {
            self.signals
                .iter()
                .rev()
                .find(|(s, _)| *s == id)
                .map(|(_, high)| *high)
        }
    }
    impl Ctx for RecCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_exec::VTime,
            _token: u64,
        ) -> Result<mosaic_exec::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }

    #[test]
    fn claim_complete_cycle() {
        let mut plic = Plic::new(PlicParams::default());
        plic.bind_output(0, SignalId(3));
        let mut ctx = RecCtx::new();
        // Source 10: priority 5, enabled.
        plic.write(&mut ctx, 4 + 10 * 4, AccessSize::Word, 5, &attr())
            .unwrap();
        plic.write(&mut ctx, ENABLE_BASE, AccessSize::Word, 1 << 9, &attr())
            .unwrap();
        plic.set_source_level(&mut ctx, 10, true);
        assert_eq!(ctx.level(SignalId(3)), Some(true));
        // Claim returns 10 and drops the line (in-progress).
        let claim = plic
            .read(&mut ctx, CONTEXT_BASE + 4, AccessSize::Word, &attr())
            .unwrap();
        assert_eq!(claim, 10);
        assert_eq!(ctx.level(SignalId(3)), Some(false));
        // Complete retires it; the still-high input re-signals.
        plic.write(&mut ctx, CONTEXT_BASE + 4, AccessSize::Word, 10, &attr())
            .unwrap();
        assert_eq!(ctx.level(SignalId(3)), Some(true));
    }

    #[test]
    fn threshold_gates_signalling() {
        let mut plic = Plic::new(PlicParams::default());
        plic.bind_output(0, SignalId(0));
        let mut ctx = RecCtx::new();
        plic.write(&mut ctx, 4 + 3 * 4, AccessSize::Word, 2, &attr())
            .unwrap();
        plic.write(&mut ctx, ENABLE_BASE, AccessSize::Word, 1 << 2, &attr())
            .unwrap();
        // Threshold 2 blocks priority 2 (strictly greater required).
        plic.write(&mut ctx, CONTEXT_BASE, AccessSize::Word, 2, &attr())
            .unwrap();
        plic.set_source_level(&mut ctx, 3, true);
        // The line is driven low by the threshold write's own update.
        assert_eq!(ctx.level(SignalId(0)), Some(false));
        // Threshold 1 lets it through.
        plic.write(&mut ctx, CONTEXT_BASE, AccessSize::Word, 1, &attr())
            .unwrap();
        assert_eq!(ctx.level(SignalId(0)), Some(true));
    }

    #[test]
    fn zero_priority_never_signals() {
        let mut plic = Plic::new(PlicParams::default());
        plic.bind_output(0, SignalId(0));
        let mut ctx = RecCtx::new();
        plic.write(&mut ctx, ENABLE_BASE, AccessSize::Word, 1 << 4, &attr())
            .unwrap();
        plic.set_source_level(&mut ctx, 5, true);
        assert_eq!(ctx.level(SignalId(0)), Some(false));
    }

    #[test]
    fn ties_break_to_the_lowest_source() {
        let mut plic = Plic::new(PlicParams::default());
        let mut ctx = RecCtx::new();
        for source in [7u32, 9] {
            plic.write(
                &mut ctx,
                4 + source as u64 * 4,
                AccessSize::Word,
                3,
                &attr(),
            )
            .unwrap();
        }
        plic.write(
            &mut ctx,
            ENABLE_BASE,
            AccessSize::Word,
            (1 << 6) | (1 << 8),
            &attr(),
        )
        .unwrap();
        plic.set_source_level(&mut ctx, 7, true);
        plic.set_source_level(&mut ctx, 9, true);
        let claim = plic
            .read(&mut ctx, CONTEXT_BASE + 4, AccessSize::Word, &attr())
            .unwrap();
        assert_eq!(claim, 7);
    }

    #[test]
    fn pending_reads_the_input_levels() {
        let mut plic = Plic::new(PlicParams::default());
        let mut ctx = RecCtx::new();
        plic.set_source_level(&mut ctx, 33, true);
        let word = plic
            .read(&mut ctx, PENDING_BASE + 4, AccessSize::Word, &attr())
            .unwrap() as u32;
        assert_ne!(word & (1 << 0), 0); // source 33 → bit 32 → word 1 bit 0
    }

    #[test]
    fn priority_is_three_bits() {
        let mut plic = Plic::new(PlicParams::default());
        let mut ctx = RecCtx::new();
        plic.write(&mut ctx, 4 + 2 * 4, AccessSize::Word, 0xF, &attr())
            .unwrap();
        let value = plic
            .read(&mut ctx, 4 + 2 * 4, AccessSize::Word, &attr())
            .unwrap();
        assert_eq!(value, 0x7);
    }
}
