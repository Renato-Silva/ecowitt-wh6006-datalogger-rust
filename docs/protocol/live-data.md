# WH6006 "Weather Logger" v1.7.3: live data, max/min, settings and device info (static RE)

Scope: live/current data, Max/Min, basic settings (units), device name, status/diagnostics (0x18),
web-server table (0x09/0x0b), mobile-network table (0x11/0x13/0x07), legacy `{..@}` commands, and unit conversions.
Everything below comes from static disassembly of `Weather Logger.exe` (`$S/ann.txt`). I did not observe any traffic.

Conventions
- "pkt[N]" means byte N of the 64-byte HID payload (report id already stripped). pkt[0]=cmd, pkt[1]=len (total bytes including cmd and len).
- Unless noted, read requests are `[cmd, 0, 0, ...]` (64 bytes, zero-filled by memset at 0x42fe00).
- Multi-byte numbers in the live block and the TLV payloads are LITTLE-endian. The app reads them as native x86 loads, or assembles them as `hi*256+lo` with lo at the lower offset.
- Confidence: H = read directly from the code; M = layout is certain but the meaning is inferred from UI labels or neighbouring code; L = a guess.

Helper scripts added: `scripts/xref.py`, `scripts/gref.py` (global xrefs), `scripts/strtab.py` (string table, output in `$S/strtab.txt`), `scripts/labels.py`, `scripts/paint.py`, `scripts/rd.py`, `scripts/vt.py` (vtables), `scripts/dlg.py` / `scripts/dlgall.py` (dialog templates, output in `$S/dialogs.txt`).

---------------------------------------------------------------------------------------------------

## 0. Main finding: cmd 0x01 is never sent; live data comes from memory reads (cmd 0x8f sub 1)

- No code path sets the expected-reply global 0x44e698 to 1, and nothing outside the dispatcher (0x4290bb..) reads the 0x01 buffer (obj+0x663c / M+0x6704). In this build the 2-packet 0x01 handler is dead code. Its layout cannot be determined. (H)
- Live data is polled by the worker thread 0x405d40, once per second (`Sleep(1000)`), but only while obj+0x3618 != 0 (set when the device reports "Firmware Ver", i.e. it is not in bootloader mode). Each pass runs:
  1. `0x4076e0`: cmd 0x80 version (firmware area; not covered here)
  2. first pass only (when obj+0x32cc is set): 0x05 (0x406b30), 0x0d (0x406cf0), 0x03 (0x406c30), 0x11/0x13/0x07 (0x407170), 0x0f (0x407620)
  3. until obj+0x32ce is set: 0x19 + 0x09 x4 + 0x0b x4 (0x406db0)
  4. **0x8f sub 1, the live block (0x4063d0)**, then 0x8f sub 2, the sensor table (0x4064e0), then cmd 0x18 status (0x4072b0)
  5. InvalidateRect, which repaints the main window (OnPaint 0x4038b0).

### 0.1 cmd 0x8f sub 1: live data block (244 bytes)
Request (0x4063d0), sent repeatedly until 0xF4 bytes have been collected:

| pkt | value | note |
|---|---|---|
| 0 | 0x8f | cmd |
| 1 | 0x06 | len |
| 2 | 0x01 | sub = live block |
| 3 | addr & 0xFF | byte offset into the block (starts at 0) |
| 4 | addr >> 8 | |
| 5 | 0x3c | requested byte count (60) |

Reply: `[0x8f, len, data[len-2]]`. The dispatcher (0x429064) appends the data to 0x44e6c0+[0x44e6a0] and adds len-2 to both [0x44e6a0] and addr (obj+0x3610). The loop exits once addr >= 0xF4 or on a 2 s timeout. 244 bytes are then copied to 0x44ef38 and decoded by **0x4066a0**. Offsets below are block offsets (blk+N). (H)

On disconnect, 0x42d4d0 fills the block with 0xFF and resets the display globals.

| blk off | size/type | scale / unit | meaning | invalid | display / evidence | conf |
|---|---|---|---|---|---|---|
| 0x00 | u32 LE | seconds since 1970 | device clock. The app calls `_gmtime64` and prints the result unchanged ("%d-%02d-%02d", "%02d:%02d:%02d"), so the value is local time encoded as epoch seconds | – | 0x4066db..0x406730 | H (meaning M) |
| 0x04 | 4 bytes | – | not read | – | – | – |
| 0x08 | u16 LE | seconds | "Learning Sensors" countdown; when nonzero the title becomes "Weather Logger \| Learning Sensors ... %d s" | 0 = normal | 0x406732 | H |
| 0x0a | u8 | % 0..100 | percentage in status-bar part 10 ("%d %%") with an icon per decile (>100 → blank). Probably logger battery level | >100 | 0x406922..0x406a80 | H layout / L meaning |
| 0x0b | u8 | – | not read | | | |
| 0x0c | u8 | – | SIM status: 0xFF → status bar part 7 shows "NO SIMCARD" | 0xFF = no SIM | 0x40682c | H |
| 0x0d | u8 | enum | cellular network type: 1="2G" 2="3G" 3="4G" 4="5G", other = blank (status part 8) | | 0x406876 | H |
| 0x0e | u8 | 0..5 | cellular signal bars (status part 9 icon index; out of range → 0) | | 0x4068e7 | H |
| 0x0f | u8 | – | not read | | | |
| 0x10 | s16 LE | /10 °C | Indoor temperature (InTemp) | 0x7FFF | → 0x44e8d0, fmt 0x42d5e0 | H |
| 0x12 | u16 LE | /10 hPa | Absolute pressure (AbsBaro) | 0xFFFF, 0x7FFF | → 0x44e8f0, fmt 0x42d7d0 | H |
| 0x14 | u16 LE | /10 hPa | Relative pressure (RelBaro) | 0xFFFF, 0x7FFF | → 0x44e8f4 | H |
| 0x16 | u8 | % | Indoor humidity | 0xFF | → 0x44e8d8, fmt 0x42e040 "%d %%" | H |
| 0x17 | u8 | % | Outdoor humidity | 0xFF | → 0x44e8dc | H |
| 0x18 | s16 LE | /10 °C | Outdoor temperature | 0x7FFF | → 0x44e8d4 | H |
| 0x1a | s16 LE | /10 °C | Dew point | 0x7FFF | → 0x44e8e0 | H |
| 0x1c | s16 LE | /10 °C | Wind chill | 0x7FFF | → 0x44e8e4 | H |
| 0x1e | s16 LE | /10 °C | Heat index | 0x7FFF | → 0x44e8e8 | H |
| 0x20 | s16 LE | /10 °C ? | a 4th derived temperature. Copied to 0x44e8ec (reset value 0x7FFF, like dew point, wind chill and heat index) but never displayed | 0x7FFF | 0x4067b0 | H layout / L meaning |
| 0x22 | u16 LE | /10 m/s | Wind speed | 0xFFFF, 0x7FFF | → 0x44e8f8, fmt 0x42d930 | H |
| 0x24 | u16 LE | /10 m/s | Gust | 0xFFFF, 0x7FFF | → 0x44e8fc | H |
| 0x26 | 2 bytes | – | not read | | | |
| 0x28 | u16 LE | degrees | Wind direction ("%d °") | 0xFFFF | → 0x44e900, fmt 0x42e040 mode 1 | H |
| 0x2a | 2 bytes | – | not read | | | |
| 0x2c | u32 LE | /10 lux | Light | 0x00FFFFFF exactly (a raw 0xFFFFFFFF is **not** treated as invalid) | → 0x44e904, fmt 0x42def0 | H |
| 0x30 | 4 bytes | – | not read | | | |
| 0x34 | u16 LE | /10 µg/m³ | PM2.5 (single value displayed; label "PM2.5") | 0xFFFF, 0x3FFF | → 0x44e96c, fmt 0x42de00 | H |
| 0x36..0x43 | – | – | not read (possibly further PM2.5 channels; unverified) | | | L |
| 0x44 | u8 | % | Soil moisture (label "Soil Moisture") | 0xFF | → 0x44e964, fmt 0x42e040 "%d %%" | H |
| 0x45..0x63 | – | – | not read | | | |
| 0x64 | s16 LE | /10 °C | extra "Temperature" sensor (label string id 0x836 "Temperature"; signal bars come from sensor-table entry 31, see 0.2) | 0x7FFF | → 0x44e970, fmt 0x42d5e0 | H |
| 0x66..0x73 | – | – | not read | | | |
| 0x74 | u8 | % | Leaf wetness (label "Leaf", shown "%d %%") | 0xFF | → 0x44e974 | H |
| 0x75..0x8f | – | – | not read | | | |
| 0x90 | u16 LE | /10 mm | value shown in the **RainHour** cell | 0xFFFFFFFF only (u16 cannot reach it, so 0xFFFF prints as 6553.5) | → 0x44e920, fmt 0x42dcc0 | H (offset), M (meaning) |
| 0x92..0xbf | – | – | not read | | | |
| 0xc0 | u16 LE | /10 mm | RainEvent | see 0x90 | → 0x44e938 | H |
| 0xc2 | u16 LE | /10 mm | RainDay | see 0x90 | → 0x44e924 | H |
| 0xc4 | u32 LE | /10 mm | RainWeek | 0xFFFFFFFF | → 0x44e928 | H |
| 0xc8 | u32 LE | /10 mm | RainMonth | 0xFFFFFFFF | → 0x44e92c | H |
| 0xcc | u32 LE | /10 mm | RainYear | 0xFFFFFFFF | → 0x44e930 | H |
| 0xd0 | u32 LE | /10 mm | RainTotal | 0xFFFFFFFF | → 0x44e934 | H |
| 0xd4 | u16 LE | /10 mm/h | RainRate (printed with a "/h" suffix) | see 0x90 | → 0x44e93c, fmt 0x42dcc0 esi=2 | H |
| 0xd6 | 2 bytes | – | not read | | | |
| 0xd8 | u16 LE | ? | copied to 0x44e908 (reset 0xFFFF) but never displayed. Probably UV radiation raw (it sits next to UVI) | 0xFFFF | 0x406813 | H layout / L meaning |
| 0xda | u8 | index | UVI ("%d") | 0xFF | → 0x44e90c, fmt 0x42e040 mode 2 | H |
| 0xdb..0xf3 | – | – | not read | | | |

Notes
- Signed temperatures are loaded with `movsx`. The formatter rejects 0x7FFF and 0xFFFF. After sign extension, a raw 0xFFFF becomes -1 and is shown as -0.1 °C, so only **0x7FFF** works as the "no data" sentinel for s16 fields. (H)
- Display grid (OnPaint 0x4038b0; label CStrings come from string-table ids via 0x41a7f0):
  row 1: InTemp, InHumi, OutTemp, OutHumi | row 2: AbsBaro, RelBaro, DewPoint, WindChill | row 3: HeatIndex, Wind Direction, Wind, Gust |
  row 4: RainHour, RainDay, RainWeek, RainMonth | row 5: RainYear, RainTotal, RainEvent, RainRate | row 6: Light, UVI, Soil Moisture, PM2.5 |
  row 7: Temperature, Leaf | row 8: Firmware Ver, Time.

### 0.2 cmd 0x8f sub 2: sensor-table rows (for signal bars only)
Request (0x4064e0): `8f 06 02 addrlo addrhi 14`, with addr = idx*20 for idx in {0,2,3,6,14,22,31,40}. Each 20-byte reply is copied to 0x44eb78+addr (a 48 x 20-byte table, 0x3c0 bytes; zeroed on reset). (H)
The main window uses only **byte 1 of each row** as a 0..5 signal-bar count (bars drawn when value >= 1..5) (H):

| row idx | drawn next to | probable sensor | conf |
|---|---|---|---|
| 6 | InTemp | indoor T/H sensor | M |
| 0 | OutTemp | outdoor array (WH24/WH65) | M |
| 2 | Wind | wind sensor | M |
| 3 | RainHour | rain sensor | M |
| 14 | Soil Moisture | WH51 CH1 | M |
| 22 | PM2.5 | WH41/43 CH1 | M |
| 31 | Temperature | extra temp sensor CH1 | M |
| 40 | Leaf | leaf sensor CH1 | M |

The rest of the 20-byte row (sensor ID and so on) is read by the Sensors-ID dialog (0x4278d0) and is outside this spec.

---------------------------------------------------------------------------------------------------

## 1. Unit conversion and display formats (shared formatters; units object at 0x44e8c8, unit bytes at +0xb4..+0xb8 = 0x44e97c..0x44e980)

| quantity | raw | unit selector (global) | conversion / format | invalid → text | evidence |
|---|---|---|---|---|---|
| Temperature | s16 /10 °C | 0x44e97c: 0=°C, ≠0=°F | °F = °C·1.8+32; "%0.1f °C" / "%0.1f °F". Mode edi=1: same without unit. Mode edi≥2: delta conversion ·1.8 with no +32 (calibration offsets) | 0x7FFF/0xFFFF → "--.-" | 0x42d5e0, consts 0x43f420=1.8, 0x43f418=32 | H |
| Pressure | u16 /10 hPa | 0x44e97d: 0=hPa, 2=inHg, other(1)=mmHg | inHg = hPa/33.86388 "%0.2f inhg"; mmHg = hPa/1.33322 "%0.1f mmhg"; hPa "%0.1f hpa" | 0xFFFF/0x7FFF → "--.-" | 0x42d7d0 | H |
| Wind/Gust | u16 /10 m/s | 0x44e97e: 0 m/s, 1 km/h (·3.6), 2 mph (·2.236936), 3 knot (·1.943844), 4 bft, else ft/s (·3.2808) | "%0.1f" + unit. Beaufort uses raw (m/s·10) thresholds: <3→0, <15→1, <34→2, <54→3, <79→4, <107→5, <138→6, <171→7, <207→8, <244→9, <284→10, <326→11, else 12 | 0xFFFF/0x7FFF → "--.-" | 0x42d930 | H |
| Rain | /10 mm | 0x44e97f: 0=mm, ≠0=inch | inch = mm/25.4 "%0.2f inch"; mm "%0.1f mm"; mode esi=2 appends "/h" | value == 0xFFFFFFFF → "--.-" | 0x42dcc0 | H |
| Light | u32 /10 lux | 0x44e980: 0=lux, 1=fc, else W/m² | fc = lux/10.76391 "%0.2f fc"; W/m² = lux/126.7 "%0.3f w/m2"; lux "%0.1f lux" | 0x00FFFFFF → "--.-" | 0x42def0 | H |
| PM2.5 | u16 /10 | – | "%0.1f ug/m3" | 0xFFFF or 0x3FFF → "--.-" | 0x42de00 | H |
| Humidity, soil, leaf | u8/u16 | – | mode 0: "%d %%" | 0xFF/0xFFFF → "--" | 0x42e040 | H |
| Wind direction | u16 | – | mode 1: "%d °" | 0xFFFF → "--" | 0x42e040 | H |
| UVI | u8 | – | mode 2: "%d" | 0xFF/0xFFFF → "--" | 0x42e040 | H |

---------------------------------------------------------------------------------------------------

## 2. cmd 0x05: read basic settings (units, timezone, interval, sensors, name)

Request (0x406b30): `05 00 00 ...` (only pkt[0] set). Reply: a single 64-byte packet copied whole (`[0x05, len, ...]`) to obj+0x673c, then to obj+0x49bc (= Setting dialog 0x9b, object obj+0x4874, at this+0x148). The poller copies pkt[2..6] to 0x44e97c..0x44e980. The dialog decodes it in **0x422650**. Combo item orders are from OnInitDialog 0x4216d0. (H)

| pkt | type | meaning | values | evidence | conf |
|---|---|---|---|---|---|
| 0 | u8 | 0x05 | | | H |
| 1 | u8 | len (= 0x12 + nameLen if it mirrors the write) | | | M |
| 2 | u8 | Temperature unit (combo 0x3ee) | 0=°C 1=°F | 0x422683, global 0x44e97c | H |
| 3 | u8 | Pressure unit (combo 0x3f1) | 0=hPa 1=mmHg 2=inHg | 0x4226ca, 0x44e97d | H |
| 4 | u8 | Wind unit (combo 0x3f0) | 0=m/s 1=km/h 2=mph 3=knots 4=bft (formatter also maps 5+ to ft/s) | 0x4226b5, 0x44e97e | H |
| 5 | u8 | Rain unit (combo 0x3f2) | 0=mm 1=inch | 0x4226df, 0x44e97f | H |
| 6 | u8 | Light unit (combo 0x3ef) | 0=lux 1=fc 2=W/m² | 0x4226a0, 0x44e980 | H |
| 7 | s8 | Time-zone index (combo 0x3f3) into the 108-entry table at 0x43ac98 (28 bytes/entry: s16 UTC offset in minutes, name ptr, DST rule from +8). 0="(UTC-12:00) International Date Line West" … 39="(UTC) Dublin, Edinburgh, Lisbon, London" … 43="(UTC+01:00) Amsterdam…" … 107="(UTC+14:00) Christmas Island" | 0..107 | 0x4226f4, 0x422730 | H |
| 8 | u8 | DST checkbox (control 0x3fd; shown only when the chosen TZ entry has a DST rule) | 0/1 | 0x422709 (BM_SETCHECK) | H layout / M meaning |
| 9..10 | u16 LE | Data-logger interval in **seconds** (shown as minutes = value/60; UI range 1..240 min) | | 0x4228d7 | H |
| 11 | u8 | Outdoor sensor type (combo 0x3f5) | 0=wh24 1=wh65 0xFF→index 2 (none) | 0x42276e | H |
| 12 | u8 | Indoor sensor (combo 0x3f7) | 0="wh30" 1/0xFF="no sensor" | 0x4227ab | H |
| 13 | u8 | WH51 soil sensor (combo 0x3fb) | 0="wh51" 1/0xFF="no sensor" | 0x4227d6 | H |
| 14 | u8 | WH41 PM2.5 sensor (combo 0x3fc) | 0="wh41" 1/0xFF="no sensor" | 0x422801 | H |
| 15 | u8 | Region (combo 0x3f8) | 0="EU" 1="USA" | 0x42282c | H |
| 16 | u8 | RF frequency (combo 0x3fa) | 0=915MHz 1=868MHz 2=433MHz 3=920MHz 4=RS485 (4 forces the three sensor combos to "no sensor" and disables them) | 0x422841..0x4228d1 | H |
| 17 | u8 | nameLen | | 0x422921 | H |
| 18.. | ASCII | Device name (control 0x3f9) | nameLen bytes | 0x422926 | H |

Matching write, **cmd 0x06** (save, 0x422b70), expected reply 0x06: `[06, 0x12+nameLen, temp, press, wind, rain, light, tz, dst, intvLo, intvHi, outdoor(2→0xFF), indoor(1→0xFF), wh51(1→0xFF), wh41(1→0xFF), region, 0x00, nameLen, name...]`. Interval = minutes·60, rejected unless 1..240 min. **pkt[16] (frequency) is always written as 0**: the app never writes the frequency. The name is converted with wcstombs. (H)

Other readers of the unit globals: the Alarm/Calibration/Rain dialogs (0x41b600, 0x41d6f0, 0x41d8c0, 0x41dad0, 0x41e100, 0x41e1c0, 0x429f80, 0x42e200/360/550) only switch unit labels. (H)

---------------------------------------------------------------------------------------------------

## 3. cmd 0x0f: Max/Min (TLV), and cmd 0x10: clear

Request (0x407620): `0f 00 00 ...`. Reply: **two packets**. Each contributes pkt[2 .. pkt[1]) and the pieces are concatenated (first, then second) at obj+0x777c. 256 bytes are copied to obj+0x4774 (Max/Min dialog 0xa0, object obj+0x46bc, this+0xb8). The copy is the **raw concatenated TLV data with no cmd/len header**. (H)

Parser **0x4209f0**: `pos=0; while pos < 0x43 { id = data[pos]; switch(id) … }`. An id outside the table (0, 0x09, 0x0a, 0x0d, 0x11–0x14, 0x18, 0x1a–0x21, >0x23) adds 0xFF to pos, which ends parsing. Values are LE (lo byte at the lower offset). In every pair the **first value is Max and the second is Min**: confirmed against dialog 0xa0, where control 0x3f6 is labelled "Max:" and 0x3f7 "Min:". (H)

| id | item size (incl. id) | layout after id | meaning (dialog group label) | invalid | → global → control | conf |
|---|---|---|---|---|---|---|
| 0x01 | 5 | s16 max, s16 min (/10 °C) | Indoor temperature | 0x7FFF | 0x44ea58/5c → 0x3f6/0x3f7 | H |
| 0x02 | 5 | s16 max, s16 min | Outdoor temperature | 0x7FFF | 0x44ea60/64 → 0x3f8/0x3f9 | H |
| 0x03 | 5 | s16 max, s16 min | Dew point | 0x7FFF | 0x44ea78/7c → 0x3fe/0x3ff | H |
| 0x04 | 3 | s16 (/10 °C) | Wind chill **Min** | 0x7FFF | 0x44ea80 → 0x405 | H |
| 0x05 | 3 | s16 | Heat index **Max** | 0x7FFF | 0x44ea84 → 0x404 | H |
| 0x06 | 3 | u8 max, u8 min (%) | Indoor humidity | 0xFF | 0x44ea68/6c → 0x3fa/0x3fb | H |
| 0x07 | 3 | u8 max, u8 min | Outdoor humidity | 0xFF | 0x44ea74/70 → 0x3fc/0x3fd | H |
| 0x08 | 5 | u16 max, u16 min (/10 hPa) | Absolute pressure | 0xFFFF | 0x44ea90/94 → 0x402/0x403 | H |
| 0x0b | 3 | u16 (/10 m/s) | Wind Max | 0xFFFF | 0x44ea98 → 0x406 | H |
| 0x0c | 3 | u16 | Gust Max | 0xFFFF | 0x44ea9c → 0x40a | H |
| 0x0e | 3 | u16 (/10 mm/h) | Rain-rate Max | 0xFFFF | 0x44eaa0 → 0x407 | H |
| 0x0f | 3 | u16 (/10 mm) | Rain-hour Max | 0xFFFF | 0x44eaa8 → 0x408 | H |
| 0x10 | 3 | u16 (/10 mm) | Rain-day Max | 0xFFFF | 0x44eaa4 → 0x409 | H |
| 0x15 | 5 | u32 LE (/10 lux) | Light Max | bytes = FF FF FF 00 | 0x44eaac → 0x40b | H |
| 0x16 | 3 | u16 | UV raw Max (parsed, not displayed) | 0xFFFF | 0x44eab0 | H layout / M meaning |
| 0x17 | 2 | u8 | UVI Max | 0xFF | 0x44eab4 → 0x40d | H |
| 0x19 | 5 | s16, s16 | unknown temperature-like pair (parsed, never displayed) | 0x7FFF | 0x44ea88/8c | H layout / L meaning |
| 0x22 | 5 | u16 max, u16 min (%) | Soil moisture | 0xFFFF | 0x44eab8/bc → 0x40c/0x40e | H |
| 0x23 | 2 | u8 (/10 µg/m³ via the PM2.5 formatter) | PM2.5 Max | – | 0x44eac0 → 0x40f | H layout / M scale |

The ids follow Ecowitt GW1000 live-data item numbering (0x01 in-temp, 0x02 out-temp, 0x03 dew, 0x04 chill, 0x05 heat, 0x06/0x07 humidity, 0x08 abs-baro, 0x0b wind, 0x0c gust, 0x0e rain-rate, 0x0f rain-hour, 0x10 rain-day, 0x15 light, 0x16 UV, 0x17 UVI). (M)
Expected item order, inferred from the clear packet below: 01,02,06,07,03,08,05,04,0b,0c,0e,0f,10,15,16,17,22,23, which totals exactly 64 data bytes. (M)

**cmd 0x10, clear Max/Min** (0x4210c0, expected reply 0x10, then "Clear Success"). Exact 45-byte request (rest zero):
```
10 2d  01 00 00  02 00 00  06 00 00  07 00 00  03 00 00  08 00 00
       05 00  04 00  0b 00  0c 00  0e 00  0f 00  10 00  15 00  16 00  17 00
       22 00 00  23 00
```
(H for bytes.) The same ids appear with one zero byte per stored value (2 for max/min pairs, 1 for single values); light 0x15 gets a single byte. Probably "id + value index flags". (M)

---------------------------------------------------------------------------------------------------

## 4. cmd 0x18: device status / diagnostics

Request (0x4072b0): `18 00 00 ...`. Reply: one packet, copied whole (`[0x18,len,...]`) to obj+0x34cc (then obj+0x338c) and to 0x44e7c0 (dispatcher struct+0x148). It is decoded by the main window (0x407380, status bar) and by the hidden "Debug Dialog" 0xb7 (thread 0x411190, which reads 0x44e7c0 every second and prints the text below). (H)

| pkt | type | meaning (debug-dialog text) | evidence | conf |
|---|---|---|---|---|
| 3..6 | u32 LE | "Current Task : 0x%08X" | 0x411252 | H |
| 7..10 | u32 LE | "Wait Task : 0x%08X" | 0x411283 | H |
| 11 | u8 | GSM state index ("ErrorCode : %s"; status bar part 6 text, 0 = blank): 0 GSM_NORMAL, 1 GSM_CGSN, 2 GSM_ATI, 3 GSM_QCFG1, 4 QCFG2, 5 QCFG3, 6 QCFG4, 7 QSCLK, 8 CPIN, 9 QINISTAT, 10 CSQ, 11 CREG, 12 COPS, 13 CFUN0, 14 CFUN1, 15 CGREG, 16 QGBAND, 17 QISEND, 18 CMGF, 19 CSCS, 20 CSMP, 21 CMGSTEL, 22 CPMS, 23 CMGR, 24 CMGD, 25 QICSGP, 26 QICLOSE, 27 QIDEACT, 28 POWERON, 29 SENDSMS, 30 QIACT, 31 DNS, 32 TCPSEND | 0x4073c6, table 0x44e3d8 (init 0x434440) | H |
| 12 | u8 | "GSM Auto Restart Times" | 0x4112fe | H |
| 13 | u8 | "GSM Error Restart Times" | 0x411324 | H |
| 14 | u8 | "WU Updata Success Times" (main: nonzero and ≠0xFF → icon in status part 2) | 0x41134a, 0x407423 | H |
| 15 | u8 | "WCLD Updata Success Times" (status part 3) | | H |
| 16 | u8 | "WOW Updata Success Times" (status part 4) | | H |
| 17 | u8 | "User Sever Updata Success Times" (status part 5) | | H |
| 18 | u8 | "WU Connect Fail Times" (the main window also uses it: nonzero and ≠0xFF → icon in status part 1) | 0x411412, 0x407451 | H |
| 19 | u8 | "WCLD Connect Fail Times" | | H |
| 20 | u8 | "SDCARD STATUS" (main stores it in obj+0x360c for the SD dialog) | 0x41142c, 0x407595 | H |
| 21 | u8 | "WOW Connect Fail Times" | | H |
| 22 | u8 | "User Connect Fail Times" | | H |
| 23 | u8 | "Time Sever Connect Fail Times" | | H |
| 24..25 | u16 LE | Battery voltage = value / 10000 V ("%0.4f") | 0x411532, const 0x43f4a0=10000 | H |

---------------------------------------------------------------------------------------------------

## 5. cmd 0x19: device name

Request (0x406db0): `19 00 00 ...` (timeout 300 ms). Reply copied whole to obj+0x6a3c. If 0 < pkt[1] < 0x14, the name is pkt[2 .. pkt[1]) (pkt[1]-2 ASCII bytes). It is stored as a CString at obj+0x5d08 and used for the log path `<dir>\<name>Log.txt` (the 0x89 debug log). (H)

## 6. cmd 0x09 / 0x0b: web-server (upload) table, 4 servers

Request (0x406db0): `09 n 00 …` and `0b n 00 …` for n = 0..3. The dispatcher stores by pkt[2]=n. Copies go to the "Web Server" dialog 0x9e (object obj+0x5148): 0x09 → this+0x148+n·0x100, 0x0b → this+0x648+n·0x100. Parser **0x425280** (A = 0x09 reply, B = 0x0b reply). (H)

0x09 reply (host):

| pkt | meaning | conf |
|---|---|---|
| 2 | server index n (0..3) | H |
| 3 | hostLen | H |
| 4.. | host / URL string (combo 0x3ee "Web"; defaults: www.ecowitt.net, rtupdate.wunderground.com, api.weathercloud.net, wow.metoffice.gov.uk, "New web") | H |

0x0b reply (parameters):

| pkt | type | meaning | conf |
|---|---|---|---|
| 2 | u8 | server index n | H |
| 3 | u8 | "Server type" combo 0x3ef: 0=jsp 1=asp 2=php | H layout / M names |
| 4 | u8 | stored as the "Upload type" combo 0x3f0 selection (Default/Customize); **bit0** also sets the global "Enable/Disable upload" combo 0x3f2. On save this byte is written from the enable flag | M |
| 5..6 | u16 LE | Port (edit 0x3f8; default "80") | H |
| 7..10 | u32 LE | Upload interval in seconds (≥3600 → shown in hours, else minutes) | H |
| 11 | u8 | idLen | H |
| 12.. | ASCII | Station ID (edit 0x3f6) | H |
| 12+idLen | u8 | pwLen | H |
| 13+idLen.. | ASCII | Password (edit 0x3f7) | H |

The save path (0x426080) builds write images `[0x0a, hostLen+4, n, hostLen, host…]` and `[0x0c, idLen+pwLen+0xd, n, srvType, enable, portLo, portHi, intv(u32 LE), idLen, id…, pwLen, pw…]`. Per CONTEXT these are sent through cmd 0x88 (upload-save area, not covered here). (H for the images)

## 7. cmds 0x11 / 0x13 / 0x07: mobile network (dialog 0x9f)

Request (0x407170): `11 00…`, `13 00…`, `07 00…` in that order. Copies go to obj+0x4230 / 0x4330 / 0x4430 (this+0x148/0x248/0x348 of dialog obj+0x40e8). Parser **0x41f090**. (H)

| reply | pkt | meaning | conf |
|---|---|---|---|
| 0x11 | 2 = apnLen, 3.. = APN (edit 0x3f6) | APN | H |
| 0x13 | 2 = userLen, 3.. = user (edit 0x3f7); then pwLen, then password (edit 0x3f8) | GPRS user/password | H |
| 0x07 | 2..3 u16 LE | SMS alarm interval, seconds (shown /60 min, edit 0x3fc) | H |
| 0x07 | 4..7 u32 LE | SMS "send current" interval, seconds (≥3600 → hours, else minutes; edit 0x3fd + combo) | H |
| 0x07 | 8 u8 | SMS ON/OFF (combo selection) | M |
| 0x07 | 9 = tel1Len (≤17, else "Tel length max 17"), 10.. tel1; then tel2Len, tel2; then tel3Len, tel3 | Tel1..3 | H |

Writes are 0x12 / 0x14 / 0x08 (0x41f520). Not decoded here.

---------------------------------------------------------------------------------------------------

## 8. Legacy `{ … @}` commands (expected reply 0x0c / 0x13): unreachable in this UI

0x40b080, 0x40e190 and 0x42b0b0 are the OK handlers, and 0x42c800 / 0x42d2d0 helpers, of dialogs **0x87 "Alarm" (156 controls), 0x86 "Calibration" and 0x81 "Setup"**. These dialogs are constructed in the main constructor (obj+0x26b0, +0x29c8, +0x2b54) but never shown: no DoModal or Create references them, and only the constructor, destructor and SEH unwind touch them. They use a different framing: payload starts with `{` (0x7B) and ends with `@}` (0x40 0x7D). They look like a leftover from an older logger. A Rust port for the WH6006 should not need them. Bytes as coded (H):
- Set interval (0x42c800): `7B 40 nn 40 7D`, nn = 5..240 (edit 0x40f); expected reply 0x13; "Set Interval Success."
- Set time (0x42d2d0, from GetLocalTime): `7B 30 YYhi YYlo MM DD hh mm ss DOW 40 7D`, expected 0x13.
- 0x40b080 (old alarm save): sends `7B 51 40 7D` ("{Q@}", expected 0x0c), then with expected 0x13: `7B 12 b b b b b b 40 7D`, `7B 14 <16 × u16 thresholds> 40 7D`, `7B 15 <19 bytes> …`, `7B 2F 40 7D` ("{/@}"). If obj+0x314 != 0 it shows "Please switch the device to the main screen." instead.
- No reply layout exists for these: the 0x0c reply is signal-only in the dispatcher, and the 0x13 reply goes to the mobile-network user/password buffer.

---------------------------------------------------------------------------------------------------

## 9. Open items / uncertainty
- cmd 0x01 layout is unknowable from this binary (dead path).
- Live block: unread regions (0x04–0x07, 0x0b, 0x0f, 0x26, 0x2a, 0x30–0x33, 0x36–0x43, 0x45–0x63, 0x66–0x73, 0x75–0x8f, 0x92–0xbf, 0xd6, 0xdb–0xf3). They probably hold extra sensor channels, but there is no evidence for that.
- blk+0x0a: percentage meaning (battery?) and blk+0x20 / blk+0xd8 meanings are guesses.
- Rain fields stored as u16 (0x90, 0xc0, 0xc2, 0xd4) can never match the formatter's 0xFFFFFFFF invalid check; treat 0xFFFF as invalid in the reimplementation (recommendation).
- Max/Min 0x23 is a single byte but is formatted /10 as PM2.5. This is odd and may be a firmware/app mismatch.
