//! This example shows how to use USB (Universal Serial Bus) in the RP2350 chip.
//!
//! This creates a WebUSB capable device that echoes data back to the host.
//!
//! To test this in the browser (ideally host this on localhost:8080, to test the landing page
//! feature):
//! ```js
//! (async () => {
//!     const device = await navigator.usb.requestDevice({ filters: [{ vendorId: 0x1EE7 }] });
//!     await device.open();
//!     await device.claimInterface(1);
//!     device.transferIn(1, 64).then(data => console.log(data));
//!     await device.transferOut(1, new Uint8Array([1,2,3]));
//! })();
//! ```

#![no_std]
#![no_main]

use core::fmt::Write as BufWrite;
use defmt::{debug, error, info, trace, warn};
use embassy_executor::Spawner;
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_rp::{
    bind_interrupts,
    gpio::{Level, Output},
    peripherals::{UART0, USB},
    uart,
    uart::BufferedUartRx,
    uart::BufferedUartTx,
    usb,
};
use embassy_time::Duration;
use embassy_usb::{
    Builder, Config,
    class::web_usb::{Config as WebUsbConfig, State, Url, WebUsb},
    driver::{Driver, Endpoint, EndpointIn, EndpointOut},
    msos::{self, windows_version},
};
use embedded_io_async::{Read, Write};
use heapless::{String, Vec};
use static_cell::{ConstStaticCell, StaticCell};
use {defmt_rtt as _, panic_probe as _};

static TX_BUF_CELL: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static RX_BUF_CELL: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);

bind_interrupts!(pub struct Irqs {
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
    UART0_IRQ => uart::BufferedInterruptHandler<UART0>;
});

// This is a randomly generated GUID to allow clients on Windows to find our device
const DEVICE_INTERFACE_GUIDS: &[&str] = &["{AFB9A6FB-30BA-44BC-9232-806CFC875321}"];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Turn on the LED to state that we have power and we are running.
    let mut led = Output::new(p.PIN_7, Level::Low);
    led.set_high();

    // Obtain the RP2350 Serial Number
    let serial = get_serial(embassy_rp::otp::get_chipid().unwrap());

    // Create the driver, from the HAL.
    let driver = usb::Driver::new(p.USB, Irqs);

    // Finger Print Sensor Setup.
    // UART
    let mut uart_config = uart::Config::default();
    uart_config.baudrate = 57600;
    uart_config.stop_bits = uart::StopBits::STOP1;
    uart_config.data_bits = uart::DataBits::DataBits8;
    uart_config.parity = uart::Parity::ParityNone;

    // safely "take" two &'static mut buffers
    let tx_buf: &'static mut [u8; 256] = TX_BUF_CELL.take();
    let rx_buf: &'static mut [u8; 256] = RX_BUF_CELL.take();

    let uart = uart::BufferedUart::new(
        p.UART0,
        Irqs,    // our bound interrupt struct
        p.PIN_0, // TX pin
        p.PIN_1, // RX pin
        tx_buf,  // TX backing buffer
        rx_buf,  // RX backing buffer
        uart_config,
    );

    // Create embassy-usb Config
    let mut config = Config::new(0x1EE7, 0x1337);
    config.manufacturer = Some("MimoCAD");
    config.product = Some("MimoFPS");
    config.serial_number = Some(serial);
    config.max_power = 500;
    config.max_packet_size_0 = 64;
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut msos_descriptor = [0; 256];

    let webusb_config = WebUsbConfig {
        max_packet_size: 64,
        vendor_code: 1,
        // If defined, shows a landing page which the device manufacturer would like the user to visit in order to control their device. Suggest the user to navigate to this URL when the device is connected.
        landing_url: Some(Url::new("https://mvac.mimocad.io/timeclock.php")),
    };

    let mut state = State::new();

    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );

    // Add the Microsoft OS Descriptor (MSOS/MOD) descriptor.
    // We tell Windows that this entire device is compatible with the "WINUSB" feature,
    // which causes it to use the built-in WinUSB driver automatically, which in turn
    // can be used by libusb/rusb software without needing a custom driver or INF file.
    // In principle you might want to call msos_feature() just on a specific function,
    // if your device also has other functions that still use standard class drivers.
    builder.msos_descriptor(windows_version::WIN8_1, 0);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));

    // Create classes on the builder (WebUSB just needs some setup, but doesn't return anything)
    WebUsb::configure(&mut builder, &mut state, &webusb_config);
    // Create some USB bulk endpoints for testing.
    let mut endpoints = WebEndpoints::new(&mut builder, &webusb_config, uart);

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    let usb_fut = usb.run();

    // Do some WebUSB transfers.
    let webusb_fut = async {
        loop {
            endpoints.wait_connected().await;
            info!("Connected");
            endpoints.relay_command().await;
        }
    };

    // Run everything concurrently.
    // If we had made everything `'static` above instead, we could do this using separate tasks instead.
    join(usb_fut, webusb_fut).await;
}

struct WebEndpoints<'d, D: Driver<'d>> {
    usb_tx: D::EndpointIn,
    usb_rx: D::EndpointOut,
    uart_tx: BufferedUartTx<'static, UART0>,
    uart_rx: BufferedUartRx<'static, UART0>,
}

impl<'d, D: Driver<'d>> WebEndpoints<'d, D> {
    fn new(
        builder: &mut Builder<'d, D>,
        config: &'d WebUsbConfig<'d>,
        uart: uart::BufferedUart<'static, UART0>,
    ) -> Self {
        let mut func = builder.function(0xff, 0x00, 0x00);
        let mut iface = func.interface();
        let mut alt = iface.alt_setting(0xff, 0x00, 0x00, None);

        // It's "IN" to the usb end point, so it's our transmitter.
        let usb_tx = alt.endpoint_bulk_in(config.max_packet_size);
        // It's "OUT" of the usb end point, so it's our receiver.
        let usb_rx = alt.endpoint_bulk_out(config.max_packet_size);
        // We split our uart interface into tx and rx parts.
        let (uart_tx, uart_rx) = uart.split();

        WebEndpoints {
            usb_tx,
            usb_rx,
            uart_tx,
            uart_rx,
        }
    }

    // Wait until the device's endpoints are enabled.
    async fn wait_connected(&mut self) {
        self.usb_rx.wait_enabled().await
    }

    async fn relay_command(&mut self) {
        let mut usb_buf = [0u8; 256];
        let mut uart_buf = [0u8; 256];
        let mut payload: Vec<u8, 256> = Vec::new();

        loop {
            match select(
                self.usb_rx.read(&mut usb_buf),
                embassy_time::with_timeout(
                    Duration::from_millis(10),
                    self.uart_rx.read(&mut uart_buf),
                ),
            )
            .await
            {
                // First is USB Side
                Either::First(Ok(n)) => {
                    let command = &usb_buf[..n];
                    pretty_print(Lvl::Info, "WebUSB -> UART", &command);

                    // Forward the command to the UART.
                    match self.uart_tx.write(command).await {
                        Ok(..) => debug!("Send to UART Successfully."),
                        Err(e) => error!("Write Error: {:?}", e),
                    };
                }
                Either::First(Err(e)) => {
                    error!("WebUSB Read Error: {}", e);
                }
                // Second is UART Side
                Either::Second(Ok(Ok(n))) => {
                    payload
                        .extend_from_slice(&uart_buf[..n])
                        .unwrap_or_else(|_| panic!("payload capacity exceeded"));

                    match whole_packet(&payload, &[0xFF, 0xFF, 0xFF, 0xFF]) {
                        Ok(len) => {
                            debug!("whole_packet len: {}", len);
                            pretty_print(Lvl::Info, "UART -> WebUSB", &payload[..len]);

                            // Send the UART reply back to the WebUSB host.
                            match self.usb_tx.write(&payload[..len]).await {
                                Ok(..) => debug!("Sent to WebUSB Successfully."),
                                Err(e) => error!("Error: {}", e),
                            };
                            pretty_print(Lvl::Debug, "WebUSB Write:", &payload[..len]);

                            let total = payload.len();
                            let remaining = total - len;

                            // slide the leftover bytes [len..total] down to the front
                            let buf = payload.as_mut_slice();
                            buf.copy_within(len..total, 0);
                            // adjust the Vec’s length
                            payload.truncate(remaining);
                        }
                        Err(..) => {}
                    };
                }
                Either::Second(Ok(Err(uart::Error::Break))) => {
                    // Normal for UART operations.
                }
                Either::Second(Ok(Err(e))) => {
                    error!("UART Error: {}", e);
                }
                Either::Second(Err(embassy_time::TimeoutError)) => {
                    // We poll UART alot, it not having data is expected.
                }
            };
        }
    }
}

#[allow(dead_code)]
enum Lvl {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
}

fn pretty_print(level: Lvl, text: &str, bytes: &[u8]) {
    // Biggest packet is 256 bytes, each byte could have 4 bytes surrounding for color codes.
    let mut buf: String<1024> = String::new();
    let len = bytes.len();

    write!(buf, "[").unwrap();
    for (idx, &b) in bytes.iter().enumerate() {
        if idx != 0 {
            write!(buf, ", ").unwrap();
        }
        match idx {
            0..2 => write!(buf, "\x1B[31m{:#04X}\x1B[0m", b).unwrap(), // Header (RED)
            2..6 => write!(buf, "\x1B[32m{:#04X}\x1B[0m", b).unwrap(), // Address (GREEN)
            6 => write!(buf, "\x1B[33m{:#04X}\x1B[0m", b).unwrap(),    // PID (YELLOW)
            7..9 => write!(buf, "\x1B[34m{:#04X}\x1B[0m", b).unwrap(), // Length (BLUE)
            9 => write!(buf, "\x1B[36m{:#04X}\x1B[0m", b).unwrap(),    // Confirmation Code (CYAN)
            idx if idx >= len.saturating_sub(2) => {
                // Checksum (MAGENTA)
                // last two bytes → checksum
                write!(buf, "\x1B[35m{:#04X}\x1B[0m", b).unwrap();
            }
            _ => write!(buf, "{:#04X}", b).unwrap(), // DATA (Uncolored)
        }
    }
    write!(buf, "]").unwrap();
    write!(buf, " ({})", buf.len()).unwrap();

    match level {
        Lvl::Trace => trace!("{=str} {=str}", text, buf.as_str()),
        Lvl::Debug => debug!("{=str} {=str}", text, buf.as_str()),
        Lvl::Info => info!("{=str} {=str}", text, buf.as_str()),
        Lvl::Warning => warn!("{=str} {=str}", text, buf.as_str()),
        Lvl::Error => error!("{=str} {=str}", text, buf.as_str()),
    }
}

/// Looks into the buffer and finds well formed data frames, returning their offset.
fn whole_packet(buffer: &[u8], address: &[u8; 4]) -> Result<usize, bool> {
    // Sanity Check (12 bytes is the smallest valid packet)
    if buffer.len() < 12 {
        debug!("Not enough data in the buffer.");
        return Err(false);
    }
    // Header
    if buffer[0..2] != [0xEF, 0x01] {
        debug!("Header Does Not Match");
        return Err(false);
    }
    // Address
    if buffer[2..6] != address[..] {
        debug!("Address Does Not Match");
        return Err(false);
    }
    // PID
    debug!("PID: {}", buffer[6]);
    // Length
    let len = usize::from_be_bytes([0, 0, buffer[7], buffer[8]]);
    debug!("LEN: {}", len);
    if len < 3 {
        debug!("Length is to short.");
        return Err(false);
    }

    // The + 9 is from the offset into the packet for the length.
    if buffer.len() < (len + 9) {
        debug!("Not a whole packet yet.");
        return Err(false);
    }

    // We should have enough data to create a whole frame.
    return Ok(len + 9);
}

/// Converts the RP2350's OPT Unique ID into a &str.
fn get_serial(unique_id: u64) -> &'static str {
    static SERIAL_STRING: StaticCell<[u8; 16]> = StaticCell::new();
    let mut serial = [b' '; 16];

    // This is a simple number-to-hex formatting
    unique_id
        .to_be_bytes()
        .iter()
        .zip(serial.chunks_exact_mut(2))
        .for_each(|(b, chs)| {
            let mut b = *b;
            for c in chs {
                *c = match b >> 4 {
                    v @ 0..10 => b'0' + v,
                    v @ 10..16 => b'A' + (v - 10),
                    _ => b'X',
                };
                b <<= 4;
            }
        });

    let serial = SERIAL_STRING.init(serial);
    let serial = core::str::from_utf8(serial.as_slice()).unwrap();

    serial
}
