//! Serial character device controllers (QEMU `hw/char/`): the MMIO and
//! port-IO UART/modem register models. The UART line fabric lives in
//! `mosaic-bus::uart`; devices behind it live in `crate::part::input`.

use crate::catalog::ModelEntry;
use crate::ip::char::adsp_sc5xx_uart::AdspSc5XxUart;
use crate::ip::char::am335x_uart::Am335XUart;
use crate::ip::char::efm32_leuart::Efm32Leuart;
use crate::ip::char::efm32_usart::Efm32Usart;
use crate::ip::char::efr32_usart::Efr32Usart;
use crate::ip::char::esp32_uart::Esp32Uart;
use crate::ip::char::esp32s3_uart::Esp32S3Uart;
use crate::ip::char::kinetis_lpuart::KinetisLpuart;
use crate::ip::char::kinetis_uart::KinetisUart;
use crate::ip::char::lpc11xx_uart::Lpc11XxUart;
use crate::ip::char::lpc17xx_uart::Lpc17XxUart;
use crate::ip::char::lpc43xx_uart::Lpc43XxUart;
use crate::ip::char::lpuart_h7::LpuartH7;
use crate::ip::char::lpuart_l4::LpuartL4;
use crate::ip::char::lx6_uart::Lx6Uart;
use crate::ip::char::max32660_uart::Max32660Uart;
use crate::ip::char::mtk_uart::MtkUart;
use crate::ip::char::ns16550a::Ns16550;
use crate::ip::char::omap_uart::OmapUart;
use crate::ip::char::pic32_uart::Pic32Uart;
use crate::ip::char::pl011::{Pl011, Pl011Params};
use crate::ip::char::ra6_uart::Ra6Uart;
use crate::ip::char::rk_uart::RkUart;
use crate::ip::char::rp2040_uart::Rp2040Uart;
use crate::ip::char::rp2350_uart::Rp2350Uart;
use crate::ip::char::rx_uart::RxUart;
use crate::ip::char::rza1_uart::Rza1Uart;
use crate::ip::char::sunxi_uart::SunxiUart;
use crate::ip::char::uart_l4::UartL4;
use crate::ip::char::usart_f1::{StUsartF1, StUsartF1Params};
use crate::ip::char::usart_f4::UsartF4;
use crate::ip::char::usart_f7::UsartF7;
use crate::ip::char::usart_h7::UsartH7;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// The unused-import marker for `SignalId` (kept for the output tables the
/// with a manifest-declared PID is spawnable: it carries no wiring beyond
/// its identity.
/// STM32F1 USART: `combined` IRQ output, `clock` input, optional console.
fn spawn_usart_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let mut usart = StUsartF1::new(
        StUsartF1Params::new(irq),
        ctx.host_uart().cloned().map(|endpoint| {
            let stream: Box<dyn mosaic_contract::stream::ByteStream> = Box::new(endpoint);
            stream
        }),
    );
    if let Some(wire) = ctx.incoming.iter().find(|wire| wire.port == "clock") {
        usart.bind_clock(wire.source, wire.signal);
    }
    machine.register_device(ctx.node, Box::new(usart))?;
    Ok(())
}
/// ARM PrimeCell UART (PL011): `combined` IRQ output, optional console.
fn spawn_pl011(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let uart = Pl011::new(
        Pl011Params::new(irq),
        ctx.host_uart().cloned().map(|endpoint| {
            let stream: Box<dyn mosaic_contract::stream::ByteStream> = Box::new(endpoint);
            stream
        }),
    );
    machine.register_device(ctx.node, Box::new(uart))?;
    Ok(())
}
/// NS16550A UART: `irq` output, optional console. Parameters: none (the
/// 16550 carries its configuration in registers, not wiring).
fn spawn_ns16550(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mut uart = Ns16550::new(irq);
    if let Some(endpoint) = ctx.host_uart() {
        uart.connect_host(endpoint.clone());
    }
    machine.register_device(ctx.node, Box::new(uart))?;
    Ok(())
}
/// Default spawn for usart-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_usart_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UsartF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for usart-f7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_usart_f7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UsartF7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for usart-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_usart_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UsartH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for uart-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_uart_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = UartL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpuart-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpuart_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = LpuartL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpuart-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpuart_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = LpuartH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc11xx-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc17xx-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc43xx-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc43xx_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc43XxUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for kinetis-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_kinetis_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = KinetisUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for kinetis-lpuart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_kinetis_lpuart(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = KinetisLpuart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pic32-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for omap-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_omap_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = OmapUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for am335x-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for ra6-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ra6_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ra6Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rx-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rx_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RxUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rza1-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rza1_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rza1Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rp2040-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rp2350-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2350_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2350Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for sunxi-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rk-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for mtk-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for efm32-leuart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efm32_leuart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efm32Leuart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for efm32-usart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efm32_usart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efm32Usart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for efr32-usart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efr32_usart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efr32Usart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for esp32-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_esp32_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Esp32Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for esp32s3-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_esp32s3_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Esp32S3Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for adsp-sc5xx-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_adsp_sc5xx_uart(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = AdspSc5XxUart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lx6-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lx6_uart(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lx6Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for max32660-uart: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_max32660_uart(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Max32660Uart::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/pl011",
        summary: "ARM PrimeCell UART (PL011) functional register model",
        outputs: &[],
        spawn: spawn_pl011,
    },
    ModelEntry {
        id: "ip:ns/ns16550a",
        summary: "NS16550A UART: FIFOs, modem lines, full register set (PC COM ports)",
        outputs: &[],
        spawn: spawn_ns16550,
    },
    ModelEntry {
        id: "ip:st/usart-f1",
        summary: "STM32F1 USART (RM0008) with clock gate and combined IRQ",
        outputs: &[],
        spawn: spawn_usart_f1,
    },
    ModelEntry {
        id: "ip:st/usart-f4",
        summary: "STM32F4 USART (RM0090) with 16-byte FIFO",
        outputs: &[],
        spawn: spawn_usart_f4,
    },
    ModelEntry {
        id: "ip:st/usart-f7",
        summary: "STM32F7 USART with FIFO and oversampling 8",
        outputs: &[],
        spawn: spawn_usart_f7,
    },
    ModelEntry {
        id: "ip:st/usart-h7",
        summary: "STM32H7 USART with FIFO and oversampling 8",
        outputs: &[],
        spawn: spawn_usart_h7,
    },
    ModelEntry {
        id: "ip:st/uart-l4",
        summary: "STM32L4 UART with low-power wakeup",
        outputs: &[],
        spawn: spawn_uart_l4,
    },
    ModelEntry {
        id: "ip:st/lpuart-l4",
        summary: "STM32L4 LPUART low-power UART",
        outputs: &[],
        spawn: spawn_lpuart_l4,
    },
    ModelEntry {
        id: "ip:st/lpuart-h7",
        summary: "STM32H7 LPUART low-power UART",
        outputs: &[],
        spawn: spawn_lpuart_h7,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-uart",
        summary: "NXP LPC11xx UART",
        outputs: &[],
        spawn: spawn_lpc11xx_uart,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-uart",
        summary: "NXP LPC17xx UART with 16-byte FIFO",
        outputs: &[],
        spawn: spawn_lpc17xx_uart,
    },
    ModelEntry {
        id: "ip:nxp/lpc43xx-uart",
        summary: "NXP LPC43xx UART",
        outputs: &[],
        spawn: spawn_lpc43xx_uart,
    },
    ModelEntry {
        id: "ip:nxp/kinetis-uart",
        summary: "NXP Kinetis UART",
        outputs: &[],
        spawn: spawn_kinetis_uart,
    },
    ModelEntry {
        id: "ip:nxp/kinetis-lpuart",
        summary: "NXP Kinetis LPUART",
        outputs: &[],
        spawn: spawn_kinetis_lpuart,
    },
    ModelEntry {
        id: "ip:microchip/pic32-uart",
        summary: "Microchip PIC32 UART",
        outputs: &[],
        spawn: spawn_pic32_uart,
    },
    ModelEntry {
        id: "ip:ti/omap-uart",
        summary: "TI OMAP UART with 64-byte FIFO",
        outputs: &[],
        spawn: spawn_omap_uart,
    },
    ModelEntry {
        id: "ip:ti/am335x-uart",
        summary: "TI AM335x UART (8250 compatible with enhanced features)",
        outputs: &[],
        spawn: spawn_am335x_uart,
    },
    ModelEntry {
        id: "ip:renesas/ra6-uart",
        summary: "Renesas RA6 SCI UART",
        outputs: &[],
        spawn: spawn_ra6_uart,
    },
    ModelEntry {
        id: "ip:renesas/rx-uart",
        summary: "Renesas RX SCI UART",
        outputs: &[],
        spawn: spawn_rx_uart,
    },
    ModelEntry {
        id: "ip:renesas/rza1-uart",
        summary: "Renesas RZ/A1 SCIF UART",
        outputs: &[],
        spawn: spawn_rza1_uart,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-uart",
        summary: "Raspberry Pi RP2040 UART",
        outputs: &[],
        spawn: spawn_rp2040_uart,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2350-uart",
        summary: "Raspberry Pi RP2350 UART",
        outputs: &[],
        spawn: spawn_rp2350_uart,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-uart",
        summary: "Allwinner Sunxi UART",
        outputs: &[],
        spawn: spawn_sunxi_uart,
    },
    ModelEntry {
        id: "ip:rockchip/rk-uart",
        summary: "Rockchip UART (8250 compatible)",
        outputs: &[],
        spawn: spawn_rk_uart,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-uart",
        summary: "MediaTek UART (8250 compatible)",
        outputs: &[],
        spawn: spawn_mtk_uart,
    },
    ModelEntry {
        id: "ip:silabs/efm32-leuart",
        summary: "Silicon Labs EFM32 LEUART low-energy UART",
        outputs: &[],
        spawn: spawn_efm32_leuart,
    },
    ModelEntry {
        id: "ip:silabs/efm32-usart",
        summary: "Silicon Labs EFM32 USART",
        outputs: &[],
        spawn: spawn_efm32_usart,
    },
    ModelEntry {
        id: "ip:silabs/efr32-usart",
        summary: "Silicon Labs EFR32 USART",
        outputs: &[],
        spawn: spawn_efr32_usart,
    },
    ModelEntry {
        id: "ip:espressif/esp32-uart",
        summary: "Espressif ESP32 UART",
        outputs: &[],
        spawn: spawn_esp32_uart,
    },
    ModelEntry {
        id: "ip:espressif/esp32s3-uart",
        summary: "Espressif ESP32-S3 UART",
        outputs: &[],
        spawn: spawn_esp32s3_uart,
    },
    ModelEntry {
        id: "ip:adi/adsp-sc5xx-uart",
        summary: "ADI ADSP-SC5xx UART",
        outputs: &[],
        spawn: spawn_adsp_sc5xx_uart,
    },
    ModelEntry {
        id: "ip:xtensa/lx6-uart",
        summary: "Cadence (Tensilica) UART for ESP32",
        outputs: &[],
        spawn: spawn_lx6_uart,
    },
    ModelEntry {
        id: "ip:maxim/max32660-uart",
        summary: "Maxim MAX32660 UART",
        outputs: &[],
        spawn: spawn_max32660_uart,
    },
];
