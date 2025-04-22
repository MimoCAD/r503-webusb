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

use defmt::{debug, error, info};
use embassy_executor::Spawner;
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_rp::{
    bind_interrupts,
    gpio::{Level, Output},
    peripherals::{UART0, USB},
    uart, usb,
};
use embassy_time::{Duration, TimeoutError};
use embassy_usb::{
    Builder, Config,
    class::web_usb::{Config as WebUsbConfig, State, Url, WebUsb},
    driver::{Driver, Endpoint, EndpointIn, EndpointOut},
    msos::{self, windows_version},
};
use heapless::Vec;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(pub struct Irqs {
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
    UART0_IRQ => uart::InterruptHandler<UART0>;
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

    let uart = uart::Uart::new(
        p.UART0,
        p.PIN_0,
        p.PIN_1,
        Irqs,
        p.DMA_CH0,
        p.DMA_CH1,
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
    ep_tx: D::EndpointIn,
    ep_rx: D::EndpointOut,
    uart: uart::Uart<'static, UART0, embassy_rp::uart::Async>,
}

impl<'d, D: Driver<'d>> WebEndpoints<'d, D> {
    fn new(
        builder: &mut Builder<'d, D>,
        config: &'d WebUsbConfig<'d>,
        uart: uart::Uart<'static, UART0, embassy_rp::uart::Async>,
    ) -> Self {
        let mut func = builder.function(0xff, 0x00, 0x00);
        let mut iface = func.interface();
        let mut alt = iface.alt_setting(0xff, 0x00, 0x00, None);

        // It's "IN" to the end point, so it's our transmitter.
        let ep_tx = alt.endpoint_bulk_in(config.max_packet_size);
        // It's "OUT" to the end point, so it's our receiver.
        let ep_rx = alt.endpoint_bulk_out(config.max_packet_size);

        WebEndpoints { ep_tx, ep_rx, uart }
    }

    // Wait until the device's endpoints are enabled.
    async fn wait_connected(&mut self) {
        self.ep_rx.wait_enabled().await
    }

    async fn relay_command(&mut self) {
        let mut buf = [0u8; 64];
        let mut read_buf: [u8; 1] = [0; 1]; // Can only read one byte at a time!

        loop {
            match select(
                self.ep_rx.read(&mut buf),
                embassy_time::with_timeout(
                    Duration::from_millis(10),
                    self.uart.read(&mut read_buf),
                ),
            )
            .await
            {
                Either::First(n) => {
                    let command = &buf[..n.unwrap()];
                    info!("Received command from host: {=[?]}", command);

                    // Forward the command to the UART.
                    match self.uart.write(command).await {
                        Ok(..) => info!("Wrote to UART"),
                        Err(e) => error!("Write Error: {:?}", e),
                    };
                }
                Either::Second(val) => {
                    match val {
                        Ok(Ok(())) => {
                            let mut data_read: Vec<u8, 255> = heapless::Vec::new(); // Save buffer.

                            loop {
                                // Some commands may need longer to get an answer from the fingerprint module.
                                // For the moement, we are just going to test with 200 ms.
                                match embassy_time::with_timeout(
                                    Duration::from_millis(10),
                                    self.uart.read(&mut read_buf),
                                )
                                .await
                                {
                                    Ok(..) => {
                                        // Extract and save read byte.
                                        let _ = match data_read.push(read_buf[0]) {
                                            Ok(..) => (),
                                            Err(e) => {
                                                error!("Unable to append {}", e);
                                                break;
                                            }
                                        };
                                    }
                                    Err(..) => break, // TimeoutError -> Ignore.
                                }
                            }
                            debug!("Read successful");

                            // Send the UART reply back to the WebUSB host.
                            match self.ep_tx.write(&data_read[..]).await {
                                Ok(..) => debug!("Sent successfully."),
                                Err(e) => error!("Error: {}", e),
                            };
                            debug!("WebUSB Write: {=[u8]}", data_read);
                        }
                        Ok(Err(e)) => {
                            error!("UART Error: {}", e);
                        }
                        Err(TimeoutError) => {
                            // We poll UART alot, it not having data is expected.
                        }
                    }
                }
            };
        }
    }
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
