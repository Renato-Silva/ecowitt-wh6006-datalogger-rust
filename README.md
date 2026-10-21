# wh6006

A Linux command-line replacement for Ecowitt's **Weather Logger** Windows app (v1.7.3), used with the **WH6006 GSM weather logger**.

The tool talks to the logger directly through `/dev/hidraw`. It has no libusb, hidapi or wine dependency.

> **Status: untested on hardware.** The protocol was reverse-engineered from the
> Windows app's disassembly (see `docs/protocol/`), not captured from a live device.
> Packet layouts follow the original code byte for byte, but nobody has run this
> against a real logger yet. Start with the read-only commands. Use `--debug` to see
> every packet, and `raw` to experiment.

## Build & install

```sh
cargo build --release
sudo install -m755 target/release/wh6006 /usr/local/bin/
# let your user open the device without root:
sudo cp 70-wh6006.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger
```

The logger enumerates as USB `04d8:003c`. Run `wh6006 devices` to check that it is detected.

## Desktop app

`wh6006-gui` covers the same commands as the CLI in a window. It has tabs for live readings with auto-refresh, max/min, settings, calibration and rain counters, alarms, mobile/SMS, weather-service upload, sensors, SD card download/delete, and device status, reboot and firmware.

The GUI is behind the `gui` feature, so the CLI build doesn't pull in the windowing dependencies:

```sh
cargo build --release --features gui
sudo install -m755 target/release/wh6006-gui /usr/local/bin/
wh6006-gui                  # --device /dev/hidrawN to pick a logger, --debug to hex-dump packets to stderr
```

It connects to the logger at startup. Settings forms only send the fields you changed, and they use the same validation as `set`. Reboot, factory reset, deleting files, overwriting rain counters and flashing firmware all ask for confirmation first.

## Usage

Read-only, safe to try first:

```sh
wh6006 info                 # firmware version, name, GSM/SD/battery status
wh6006 live                 # current readings (in the logger's units; --metric to force metric)
wh6006 live --watch 10      # refresh every 10 s; add --json for machine-readable output
wh6006 maxmin
wh6006 settings get
wh6006 calibration get
wh6006 alarms get
wh6006 mobile get
wh6006 upload get
wh6006 sensors get
wh6006 sd list
wh6006 sd download --all -o ~/weather    # CSV history files from the SD card
wh6006 check-updates        # compares with Ecowitt's published firmware
wh6006 log                  # live device log: the modem's AT commands and replies (e.g. SIM errors)
```

Changing settings: every `set` command takes `key=value` pairs. The keys are the ones printed by the matching `get`. The tool reads the current values, changes only the keys you give, and writes the result back.

```sh
wh6006 settings set interval=5 timezone=45 dst=on temp_unit=C name=Garden
wh6006 timezones                         # index list for timezone=
wh6006 time sync                         # set the clock from the PC (sent as UTC)
wh6006 calibration set out_temp=-0.3 rel_pressure=12.5 rain_gain=1.05
wh6006 alarms set out_temp_low=0 out_temp_low_on=on wind=15 wind_on=on
wh6006 rain day=0 week=12.4              # mm; omitted counters are left unchanged
wh6006 mobile set apn=internet sms=on phone1=+351900000000 alarm_interval_min=30
wh6006 mobile test-sms
wh6006 upload set wu enabled=on station_id=IXXXX password=secret interval_min=5
wh6006 upload set custom host=wx.example.org port=80 station_id=me password=key type=php
wh6006 sensors set 6 learn               # re-register a sensor (or give a hex ID)
wh6006 maxmin --clear
wh6006 sd delete 20260101.csv
wh6006 reset                             # reboot   (--factory for factory reset)
wh6006 firmware update WH6006CodeV1.1.39.hex
```

Upload settings are saved as one table, as in the Windows app. The firmware turns all uploads off when `interval_min=0`, and it skips any service whose station ID or password is empty. The custom server receives Wunderground-style `GET /weatherstation/updateweatherstation.<type>?ID=…&PASSWORD=…` requests (see `docs/protocol/device-firmware.md`). Every `upload set` rewrites all four servers. `enabled` and `interval_min` are shared by all servers, and the other keys only change the server you name.

Values for `set` are always metric: °C, hPa, m/s, mm and %. Use `none` to clear an alarm threshold.

### Firmware update

`firmware update` reproduces the original app's bootloader sequence exactly:

1. Send 86, then check that 80 reports "Bootloader".
2. Erase with 82.
3. Program with 2890 × 83 packets, 56 bytes each, covering PC 0x1800–0x153FC.
4. Finish with 84.
5. Read every chunk back with 85 and compare.
6. Start the application with 87.

The bootloader region is never touched. Even so, flashing a protocol that hasn't been tested on hardware carries real risk. Keep the Windows app available as a fallback. The current `.hex` URL is printed by `check-updates`.

### Debugging

```sh
wh6006 --debug info                # hex dump of every packet
wh6006 raw 80                      # send bytes, print the reply
wh6006 raw 8f 06 01 00 00 3c       # first 60 bytes of the live block
```

Unsolicited device log lines (cmd 0x89) appear in `--debug` output.

## Differences from the Windows app

- **No graphs and no Access DB.** History lives in the CSV files on the SD card. `sd download` saves them byte for byte (LF line endings), so they open in any spreadsheet or plotting tool. `--resume` continues a partial download.
- **Settings save:** the Windows app always wrote 0 to the radio-frequency byte. This tool writes back the value it read.
- **Intel HEX parsing:** start-address records (types 03 and 05) no longer corrupt the extended address, which was a bug in the app. Gaps in the image are filled with 0xFF instead of uninitialised memory.
- **Not ported:** the legacy `{…@}` dialogs, which are unreachable in v1.7.3; the language switch; and the PC-software update prompt (`check-updates` reports firmware only).

## Protocol summary

The logger uses 64-byte HID reports with no checksum: `[cmd, length, data…]`, zero padded. Full details, with evidence addresses and confidence levels, are in `docs/protocol/`.

| cmd | purpose |
|---|---|
| 80 | version (`"Firmware Ver:x.y.z"` / `"Bootloader Ver:…"`) |
| 8f 06 01 a a 3c | live data block (244 bytes, read in 60-byte chunks) |
| 8f 06 02 a a 14 | sensor slot record (20 bytes) · 91 = set sensor ID |
| 18 | status / diagnostics |
| 0f / 10 | max/min (two-packet TLV) / clear |
| 05 / 06 | settings read / write |
| 02 | set items: time (tag 18) or rain counters (tags 10–14) |
| 03 / 04 | calibration |
| 0d / 0e | alarms |
| 11/12, 13/14, 07/08 | APN, GPRS user/password, SMS + phones |
| 09/0a, 0b/0c | upload server host / parameters (sub 0..3) |
| 15 / 16 / 17 | SD list / download / delete |
| 19 | device name |
| 86, 87, 82–85 | bootloader: enter, run app, erase, program, done, verify |
| 88 / 8a / 8e | factory reset / test SMS / sync network time |
