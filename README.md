# Pinouts

## Adafruit Feather RP2350 HSTX
![Adafruit Feather RP2350 HSTX Pinout](https://raw.githubusercontent.com/adafruit/Adafruit-Feather-RP2350-PCB/refs/heads/main/Adafruit_Feather_RP2350_prettypins.svg)

## Connections

| # | R503 Wire Color                        | Function |  PIN#  | Pin Name                 | Notes                                 |
| - |                  :---:                 |   :---:  |  :---: |           :---:          |                 :---:                 |
| 1 | $${\color{red}RED \space Wire}$$       | Power    |   3V3  | $${\color{red}3V3}$$     | 3.3 Volts (Top Left Of Feather)       |
| 2 | $${\color{black}BLACK \space Wire}$$   | Ground   |   GRD  | $${\color{black}GND}$$   | Ground (Right Under 3.3 Pins)         |
| 3 | $${\color{yellow}YELLOW \space Wire}$$ | TX (Out) |    01  | $${\color{green}RX}$$    | TX on R503 becomes RX on the Feather. |
| 4 | $${\color{green}GREEN \space Wire}$$   | RX (In)  |    02  | $${\color{yellow}TX}$$   | RX on R503 becomes TX on the Feather. |
| 5 | $${\color{blue}BLUE \space Wire}$$     | Wakeup   |    03  | $${\color{BLUE}GPIO 3}$$ | For Touch Sense                       |
| 6 | $${\color{white}WHITE \space Wire}$$   | Touch    |   3V3  | $${\color{red}3V3}$$     | 3.3 Volts (Top Left Of Feather)       |

## Grow R503 Pro
![GROW R503 Pro MX1.0-6P Pinout](https://probots.co.in/pub/media/wysiwyg/GROW_R503_-5.jpg)

# Software
# [Rust](https://rust-lang.org)
You'll need at least Rust 1.75.0 to compile this. That is 1.75 is the MSRV or Minimum Supported Rust Version.
You'll also need to run `rustup target add thumbv8m.main-none-eabihf` so you can cross compile for the RP2350 chip. Don't worry, in Rust cross compiling is easy.

# [Probe-rs](https://probe.rs).
Probe-rs is super cool and you'll need to install that in order to be able to flash the board over it's SWD header. You connect the SWD port of the Feather to the Raspberry Pi Debug Probe's `U` connector. Hardware needed for that is below.

# Hardware
 * [Raspberry Pi Debug Probe](http://adafru.it/5699)
 * [Adafruit Feather RP2350 with HSTX Port](http://adafru.it/6000)
 * [GROW R503 Pro Fingerprint Sensor](https://en.hzgrow.com/product/204.html)
 * Pololu JST SH-Style Connector [Top-Entry](https://www.pololu.com/product/4771) or [Side-Entry](https://www.pololu.com/product/4773)
 * [Dupont Jumper Cables M/M](http://adafru.it/759)
 * [Soldering Iron](https://www.adafruit.com/category/559)
