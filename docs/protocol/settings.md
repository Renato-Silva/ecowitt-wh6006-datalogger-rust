# WH6006 HID protocol: configuration read/write commands

Source: static disassembly of "Weather Logger.exe" v1.7.3 (`$S/ann.txt`). Dialog control IDs and labels come from the RT_DIALOG resources, parsed with `$S/scripts/cfg_dlgres.py` into `$S/dialogs.txt`.
Confidence levels: **H** = read directly from the code, **M** = strong inference (for example a label mapping or matching naming), **L** = guess. Fields marked "unknown" were not resolved.

Notation: `p[i]` is byte i of the 64-byte HID payload (the report-ID byte 0 is not counted). All multi-byte values are little-endian unless noted. The one exception is the year in the time item, which is big-endian.

---

## 0. Framing, length byte, checksum (applies to everything below)

| Item | Finding | Evidence | Conf |
|---|---|---|---|
| Checksum | **None.** `0x429b00` memcpy's the caller's 64 bytes after a 0x00 report ID and calls `WriteFile(h, buf, 0x41)`. No byte is computed over the payload. Every save handler fills only cmd, len and data. | 429b28-429b75 | H |
| Unused bytes | Zero. Every sender memsets the buffer before filling it (`memset(p+1,0,0x3f)`). | e.g. 422ec1 | H |
| `p[1]` for writes | = total number of meaningful bytes **including** `p[0]` and `p[1]`. Examples: time `02 0a ...` (10 bytes), rain `02 19 ...` (25), calib `04 19` (25), alarm `0e 31/33`, settings `06 (0x12+n)`, sensor `91 07`. **Exception (app quirk):** `0x12` (APN) sets `p[1] = n+2` although the packet is `n+3` bytes long, see §6. | various | H |
| `p[1]` for simple reads | 0 (request is just `[cmd]`) for 0x03, 0x05, 0x07, 0x0d, 0x11, 0x13, 0x19. For 0x09/0x0b, `p[1]` is a **sub-index** (0..3), not a length. | 406b71, 406e53-406f93 | H |
| Reply cmd | The app sets the expected-reply global `0x44e698` to the same value as the command for most writes (0x02, 0x04, 0x06, 0x0e, 0x12, 0x14, 0x08, 0x8a, 0x8e, 0x91, 0x86). Exceptions: web-server writes 0x0a/0x0c and factory reset 0x88 expect **0x88**. Test commands 0x8b/0x8c expect 0x08. The app never inspects the content of a write reply. It only waits for the event (timeout 0x1f4-0x3e8 ms). The alarm save is the one exception: on timeout (WAIT_TIMEOUT 0x102) it shows "Save Fail". | 41ccaa | H |
| Value sentinels | The app sends "blank/--" fields as all-ones: `0xFF` (u8), `0xFFFF` (u16), `0xFFFFFFFF` (u32). In alarm **replies**, an unset temperature is `0x7FFF` (bytes `ff 7f`). | 42e5c0, 41bfe4 | H |

### Shared "item tag" namespace (TLV items in 0x02 / 0x03-0x04 / 0x0d-0x0e / 0x10)
Several commands carry `tag, value...` items. The same tag numbers are used everywhere, and they match Ecowitt's well-known GW1000 live-data item IDs (inference **M**):

| tag | meaning (as used here) | tag | meaning |
|---|---|---|---|
| 0x01 | Indoor temp | 0x0b | Wind speed |
| 0x02 | Outdoor temp | 0x0c | Gust |
| 0x03 | Dew point | 0x0d | Rain (event); used for **rain gain** in calibration |
| 0x04 | (wind chill?) only in 0x10 | 0x0e | Rain rate |
| 0x05 | (heat index?) only in 0x10 | 0x10 | Rain day |
| 0x06 | Indoor humidity | 0x11 | Rain week |
| 0x07 | Outdoor humidity | 0x12 | Rain month |
| 0x08 | Absolute pressure | 0x13 | Rain year |
| 0x09 | Relative pressure | 0x14 | Rain total |
| 0x0a | Wind direction | 0x15 | Light; used for **light gain** in calibration |
| 0x18 | Date/time | 0x21 | Master alarm on/off (alarm only) |

### Global unit bytes (from 0x05 reply p[2..6], cached in 0x44e97c..0x44e980)
The app converts every user-entered value to device units, using the current units: temp `0x44e97c=p[2]`, pressure `0x44e97d=p[3]`, wind `0x44e97e=p[4]`, rain `0x44e97f=p[5]`, light `0x44e980=p[6]`. **Device units are always: 0.1 °C, 0.1 hPa, 0.1 m/s, 0.1 mm, degrees, % RH.** (H; constants 10.0 @43f3d8, 1.8 @43f420, 32 @43f418, 33.86388 @43f410, 1.33322 @43f408, 3.6 @43f400, 2.236936 @43f3f0, 1.943844 @43f3f8, 25.4 @43f3e0, 100.0 @43f488)

---

## 1. Time sync: `0x02` with item 0x18 (func 0x422eb0, "Sync Time" button 0x43e in Setting dialog)

`0x02` is a generic **"write item values"** command that takes TLV items. Time sync and the rain-total editor differ only in the items they carry (tag 0x18 vs tags 0x10-0x14).

Request (10 bytes):

| off | size | type | value / meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x02 | 422f0f | H |
| 1 | 1 | u8 | 0x0a (length) | 422f1c `WORD 0x180a` | H |
| 2 | 1 | u8 | 0x18 item tag "date/time" | 422f1c | H |
| 3 | 1 | u8 | year **high** byte (`wYear>>8`) | 422ee8/422ef1/422f09 | H |
| 4 | 1 | u8 | year **low** byte (year is full value, e.g. 2026 = `07 ea`, **big-endian**, no offset) | 422eea | H |
| 5 | 1 | u8 | month 1-12 (binary, not BCD) | 422eed-422efb | H |
| 6 | 1 | u8 | day 1-31 | 422ee4/422f02 | H |
| 7 | 1 | u8 | hour 0-23 | 422f0c/422f13 | H |
| 8 | 1 | u8 | minute | 422efe/422f16 | H |
| 9 | 1 | u8 | second | 422f05/422f19 | H |

* The time source is `GetSystemTime()`, i.e. **UTC**, not local time. No timezone/DST field is sent here. The device applies the timezone index and DST flag from the settings (0x06). Day-of-week and milliseconds are not sent. (H)
* Expected reply: 0x02. On completion the app shows "Sync Time Success" without checking the reply. (H)

Example (2026-10-08 12:34:56 UTC): `02 0a 18 07 ea 0a 08 0c 22 38`

## 2. Rain totals: `0x02` with items 0x10-0x14 (func 0x42a1c0, dialog 165 "Rain")

Request (25 bytes, `p[1]=0x19`):

| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x02 | 42a305 | H |
| 1 | 1 | u8 | 0x19 | 42a3c3 | H |
| 2 | 1 | u8 | tag 0x10 (Rain day) | 42a309 | H |
| 3 | 2 | u16 LE | RainDay, 0.1 mm, 0..9999 (999.9 mm) | 42a318-42a32d | H |
| 5 | 1 | u8 | tag 0x11 (Rain week) | 42a33a | H |
| 6 | 4 | u32 LE | RainWeek, 0.1 mm, 0..99999 | 42a349 + 42e720 | H |
| 10 | 1 | u8 | tag 0x12 (Rain month) | 42a35d | H |
| 11 | 4 | u32 LE | RainMonth, 0.1 mm, 0..99999 | 42a36e | H |
| 15 | 1 | u8 | tag 0x13 (Rain year) | 42a380 | H |
| 16 | 4 | u32 LE | RainYear, 0.1 mm, 0..99999 | 42a391 | H |
| 20 | 1 | u8 | tag 0x14 (Rain total) | 42a3a3 | H |
| 21 | 4 | u32 LE | RainTotal, 0.1 mm, 0..99999 | 42a3b4 | H |

* Encoding (`0x42e550`): `round(atof(text) * 10 [* 25.4 if rain unit = inch])`, truncated after +0.5. A blank or "--" field gives 0xFFFF; the u32 helper `0x42e720` then writes `ff ff ff ff`, and the u16 day field gets `ff ff`. **Use the sentinel to leave a counter unchanged** (M: the device semantics are inferred). (H for encoding)
* UI range strings: "RainDay: 0 to 999.9 mm", the others "0 to 9999.9 mm" (but the code limit is 99999 = 9999.9 mm). (H)
* Expected reply: 0x02. Read side: the dialog fills from the live-data decode (globals 0x44effa..0x44f008 copied at 406a82). There is **no dedicated rain read command**; the values come from the live-data stream (cmd 0x01 items 0x10-0x14, see live-data spec). (H for source, M that it is cmd 0x01)

---

## 3. Basic settings: read `0x05`, write `0x06` (dialog 155 "Setting"; save 0x422b70, fill 0x422650)

Read request: `05` (rest 0). The reply is stored at obj+0x673c, copied to obj+0x49bc (= Setting-dialog+0x148), and its bytes 2..6 are copied to the unit globals.
The write has the **same layout** as the read reply (the fill routine 422650 reads the same offsets that the save writes), with two differences noted in the table.

| off | size | type | meaning / values | write source (422b70) | read use (422650) | conf |
|---|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x05 (reply) / 0x06 (write) | 422dc9 | - | H |
| 1 | 1 | u8 | length = 0x12 + name_len | 422dc6 `lea edx,[ebx+0x12]` | - | H |
| 2 | 1 | u8 | **Temperature unit** 0=°C, 1=°F | combo 0x3ee (CB_GETCURSEL) 422bc5 | 422683 | H |
| 3 | 1 | u8 | **Pressure unit** 0=hPa, 1=mmHg, 2=inHg | combo 0x3f1 → 422c04 | 4226ca | H |
| 4 | 1 | u8 | **Wind unit** 0=m/s, 1=km/h, 2=mph, 3=knots, 4=bft | combo 0x3f0 → 422be3 | 4226b5 | H |
| 5 | 1 | u8 | **Rain unit** 0=mm, 1=inch | combo 0x3f2 → 422c19 | 4226df | H |
| 6 | 1 | u8 | **Light unit** 0=lux, 1=fc, 2=W/m² | combo 0x3ef → 422bda | 4226a0 | H |
| 7 | 1 | s8 | **TimeZone index** 0..107 into the app's table (Appendix A). Read with `movsx` | combo 0x3f3 → 422c22 | 4226f4 | H (index), M (device uses same table) |
| 8 | 1 | u8 | **DST** auto-adjust: 0=off, 1=on (checkbox 0x3fd "Automatically adjust clock for daylight saving time"). The app greys the checkbox out for zones with no DST rule | BM_GETCHECK 422c43 | BM_SETCHECK 422709 | H |
| 9 | 2 | u16 LE | **Logger interval in seconds** = minutes × 60. UI: 1..240 min ("Data Logger Interval: 1 - 240 Minute") | 422d20-422d48 | 4228d7 (÷60) | H |
| 11 | 1 | u8 | **Outdoor sensor**: 0=WH24, 1=WH65, 0xFF=none (UI index 2 ↔ 0xFF) | combo 0x3f5 422c46-422c66 | 42276e | H |
| 12 | 1 | u8 | **Indoor sensor**: 0=WH30, 0xFF=no sensor (UI idx1 ↔ 0xFF). Control is hidden in this build | combo 0x3f7 422c7b | 4227ab | H |
| 13 | 1 | u8 | **Soil sensor**: 0=WH51, 0xFF=no sensor (hidden control) | combo 0x3fb 422cac | 4227d6 | H |
| 14 | 1 | u8 | **PM2.5 sensor**: 0=WH41, 0xFF=no sensor (hidden control) | combo 0x3fc 422cdd | 422801 | H |
| 15 | 1 | u8 | **Area**: 0=EU, 1=USA (hidden control "Area:") | combo 0x3f8 422cfc | 42282c | H (value), M (meaning) |
| 16 | 1 | u8 | **Frequency** (read): 0=915 MHz, 1=868 MHz, 2=433 MHz, 3=920 MHz, 4=RS485. If 4 (RS485), the app forces indoor/soil/PM2.5 to "no sensor" and disables them. **On write the app always sends 0** (the combo is display-only and disabled) | write: `mov [ebp-0x40],0` 422d1c | 422841-4228d1 | H |
| 17 | 1 | u8 | Device name length n | 422dd0 | 422921 | H |
| 18 | n | ASCII | **Device name** (no NUL; wide → `wcstombs`) | 422d70-422db7 | 422926-42295f | H |

Notes
* The combo "DST:" 0x3f4 (off/on) is hidden and never read or sent. DST is p[8] from the checkbox. (H)
* The app sets no length limit on the name. Packet space allows n ≤ 46. The app's temp buffer (ebp-0x70, 33 bytes) is safe only for n ≤ 31. **Recommend n ≤ 31.** (H for code, M for limit)
* Device name can also be read with `19` → reply `19 <len> <name...>`. The string is `p[2..p[1])`, so name length = p[1]-2. (H, from CONTEXT dispatcher)
* Reply to 0x06: 0x06, contents ignored. (H)
* Since the app always writes freq=0, a reimplementation should **echo back the value read in p[16]**, and the other hidden fields too (p[12..15]). Whether the device honours p[16] is unknown. (M)
* Settings-dialog test buttons (hidden): "TESTCURRENT" sends `8b` (1 byte), "TESTRFCURRENT" sends `8c`, both with expected reply 0x08 (funcs 0x422fc0 / 0x423070). Factory test commands; meaning unknown. (H bytes, M button mapping)

---

## 4. Calibration: read `0x03`, write `0x04` (dialog 156 "Calibration"; save 0x41e320, parse 0x41ded0, encode 0x41d8c0)

Read request: `03` (rest 0). The reply is copied to obj+0x3fa8, and the parser walks TLV items from p[2] while offset < 0x17 (bytes 2..24).
The write has the identical TLV layout, `p[1]=0x19` (25 bytes):

| off | size | type | tag / meaning | scale & range (UI validation) | write evidence | read evidence | conf |
|---|---|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x03 / 0x04 | | 41e614 | | H |
| 1 | 1 | u8 | 0x19 | | 41e761 | | H |
| 2 | 1 | u8 | tag 0x01 | | 41e618 | jt 0x41e0dc | H |
| 3 | 1 | **s8** | Indoor temp offset | 0.1 °C, -60..+60 (±6.0 °C / ±10.8 °F); °F input ÷1.8 | 41e62a (type1) | → 0x44e988 movsx | H |
| 4 | 1 | u8 | tag 0x02 | | 41e63e | | H |
| 5 | 1 | s8 | Outdoor temp offset | same as above | 41e64b | → 0x44e989 | H |
| 6 | 1 | u8 | tag 0x06 | | 41e65f | | H |
| 7 | 1 | s8 | Indoor humidity offset | 1 %RH, -50..+50 | 41e66e (type0, wtoi) | → 0x44e98a | H |
| 8 | 1 | u8 | tag 0x07 | | 41e682 | | H |
| 9 | 1 | s8 | Outdoor humidity offset | 1 %RH, -50..+50 | 41e68f | → 0x44e98b | H |
| 10 | 1 | u8 | tag 0x08 | | 41e6a0 | | H |
| 11 | 2 | **s16 LE** | Absolute pressure offset | 0.1 hPa, -8000..+8000 (±800 hPa / ±600 mmHg / ±23.6 inHg) | 41e6b2 (type2) | → 0x44e98c (`[1]+[2]*256`, movsx) | H |
| 13 | 1 | u8 | tag 0x09 | | 41e6c3 | | H |
| 14 | 2 | s16 LE | Relative pressure offset | same | 41e6d0 | → 0x44e98e | H |
| 16 | 1 | u8 | tag 0x0a | | 41e6e1 | | H |
| 17 | 2 | s16 LE | Wind direction offset | 1°, -180..+180 | 41e6f3 (type3, wtoi) | → 0x44e990 | H |
| 19 | 1 | u8 | tag 0x0b | | 41e704 | | H |
| 20 | 1 | u8 | Wind gain | ×0.01, 50..150 (0.50..1.50) | 41e716 (type4) | → 0x44e992, shown /100 | H |
| 21 | 1 | u8 | tag 0x0d | | 41e727 | | H |
| 22 | 1 | u8 | Rain gain | ×0.01, 50..150 | 41e734 | → 0x44e993 | H |
| 23 | 1 | u8 | tag 0x15 | | 41e739 | | H |
| 24 | 1 | u8 | Light gain | ×0.01, 50..150 | 41e752 | → 0x44e994 | H |

Notes
* Write encoders **truncate** toward zero (x87 RC=chop, `or eax,0xc00`). The validators round. A reimplementation should round. (H)
* Display: a u8 value 0xFF shows "--" for humidity and gains. (H, 41d734)
* The read parser ignores unknown tags 3, 4, 5, 0x0c, 0x0e-0x14 and stops on any other unknown tag (adds 0xff to the index). (H)
* **No per-channel CH1..CH8 calibration exists in the WH6006 command set.** Dialog 134 ("Calibration", CH1..CH8 temp/humi) belongs to a **different, legacy brace-framed protocol**, see §11. (H)

---

## 5. Alarms: read `0x0d`, write `0x0e` (dialog 157 "Alarm"; save 0x41c420, parse 0x41bfa0)

Read request: `0d` (rest 0). The reply is copied to obj+0x36d8. `p[1]` is 0x31 (no master-alarm item) or 0x33 (with item 0x21). The dispatcher sets a flag "master alarm supported" = `p[1] != 0x31`, and the parser walks `p[1]-2` bytes from p[2].
The write has the same layout. Each item is `tag, hi_value, hi_enable, lo_value, lo_enable`, or `tag, value, enable` for single-threshold items:

| off | size | type | meaning | scale / range | evidence (write) | read global | conf |
|---|---|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x0d / 0x0e | | 41c936 | | H |
| 1 | 1 | u8 | 0x31, or 0x33 if master alarm supported | | 41cc27-41cc4c | | H |
| 2 | 1 | u8 | tag 0x01 Indoor temp | | 41c93a | | H |
| 3 | 2 | s16 LE | Indoor temp **high** alarm | 0.1 °C, -400..600 (-40.0..60.0 °C); °F: (F-32)/1.8 | 41c944-41c95e (42e200) | 0x44ea0c | H |
| 5 | 1 | u8 | high enable 0/1 (checkbox 0x3f5) | | 41c974 | 0x44ea44 | H |
| 6 | 2 | s16 LE | Indoor temp **low** alarm | same | 41c97d-41c997 | 0x44ea10 | H |
| 8 | 1 | u8 | low enable (0x404) | | 41c9b2 | 0x44ea45 | H |
| 9 | 1 | u8 | tag 0x06 Indoor humidity | | 41c9b5 | | H |
| 10 | 1 | u8 | Indoor humi high | 1..99 %, 0xFF=unset | 41c9bf/41c9d2 (42e640) | 0x44ea1c | H |
| 11 | 1 | u8 | high enable (0x3fd) | | 41c9cc | 0x44ea48 | H |
| 12 | 1 | u8 | Indoor humi low | 1..99 % | 41c9eb | 0x44ea20 | H |
| 13 | 1 | u8 | low enable (0x406) | | 41ca00 | 0x44ea49 | H |
| 14 | 1 | u8 | tag 0x02 Outdoor temp | | 41ca03 | | H |
| 15 | 2 | s16 LE | Outdoor temp high | 0.1 °C, -40..60 °C | 41ca0d-41ca27 | 0x44ea14 | H |
| 17 | 1 | u8 | high enable (0x40b) | | 41ca3d | 0x44ea46 | H |
| 18 | 2 | s16 LE | Outdoor temp low | | 41ca46-41ca60 | 0x44ea18 | H |
| 20 | 1 | u8 | low enable (0x40f) | | 41ca78 | 0x44ea47 | H |
| 21 | 1 | u8 | tag 0x07 Outdoor humidity | | 41ca7b | | H |
| 22 | 1 | u8 | Outdoor humi high | 1..99 % | 41ca98 | 0x44ea24 | H |
| 23 | 1 | u8 | high enable (0x40c) | | 41ca92 | 0x44ea4a | H |
| 24 | 1 | u8 | Outdoor humi low | | 41cab1 | 0x44ea28 | H |
| 25 | 1 | u8 | low enable (0x410) | | 41cac6 | 0x44ea4b | H |
| 26 | 1 | u8 | tag 0x03 Dew point | | 41cac9 | | H |
| 27 | 2 | s16 LE | Dew point high | 0.1 °C, -40..60 °C | 41cad3-41caed | 0x44ea2c | H |
| 29 | 1 | u8 | high enable (0x3fc) | | 41cb03 | 0x44ea4c | H |
| 30 | 2 | s16 LE | Dew point low | | 41cb0c-41cb26 | 0x44ea30 | H |
| 32 | 1 | u8 | low enable (0x405) | | 41cb36 | 0x44ea4d | H |
| 33 | 1 | u8 | tag 0x0b Wind | | 41cb39 | | H |
| 34 | 2 | u16 LE | Wind speed high alarm | 0.1 m/s, 0..500 (50.0 m/s) | 41cb43-41cb5d (42e360) | 0x44ea34 | H |
| 36 | 1 | u8 | enable (0x3fe) | | 41cb73 | 0x44ea4e | H |
| 37 | 1 | u8 | tag 0x0c Gust | | 41cb76 | | H |
| 38 | 2 | u16 LE | Gust high alarm | 0.1 m/s, 0..500 | 41cb80-41cb9a | 0x44ea38 | H |
| 40 | 1 | u8 | enable (0x3ff) | | 41cbb0 | 0x44ea4f | H |
| 41 | 1 | u8 | tag 0x0e Rain rate | | 41cbb3 | | H |
| 42 | 2 | u16 LE | Rain-rate high alarm | 0.1 mm/h, 0..9999 | 41cbbd-41cbd7 (42e550) | 0x44ea3c | H |
| 44 | 1 | u8 | enable (0x400) | | 41cbed | 0x44ea50 | H |
| 45 | 1 | u8 | tag 0x10 Daily rain | | 41cbf0 | | H |
| 46 | 2 | u16 LE | Daily-rain high alarm | 0.1 mm, 0..9999 | 41cbfa-41cc14 | 0x44ea40 | H |
| 48 | 1 | u8 | enable (0x408) | | 41cc24 | 0x44ea51 | H |
| 49 | 1 | u8 | tag 0x21 **Master alarm** (only if supported, len 0x33) | | 41cc34 | | H |
| 50 | 1 | u8 | 0=Disable, 1=Enable (combo 0x3ee "Master Alarm On:") | | 41cc38-41cc3d | dlg+0x885 (bool) | H |

Notes
* Write encoders: temp `0x42e200` (`round(v*10)` or `round((F-32)/1.8*10)`). Wind `0x42e360`: m/s ×10; km/h ×10/3.6; mph ×10/2.236936; knots ×10/1.943844; bft maps 0..12 to 0,3,15,34,54,79,107,138,171,207,244,326 (0.1 m/s; ≥11 → 326). Other unit index → ft/s ×10/3.2808. Rain `0x42e550`. Humidity `0x42e640(…,0)` = wtoi, "--" gives 0xFF. A blank field is sent as 0xFFFF / 0xFF. (H)
* Read side: a temperature (or wind/gust/rain) value `ff 7f` (0x7FFF) means unset. For wind/rain the check is `ff ff`. Humidity 0xFF means unset. Enable bytes are treated as bool (`!=0`). (H)
* Tag handling in the read parser: 0x01/0x02/0x03 = 7 bytes (tag + s16 + en + s16 + en), 0x06/0x07 = 5 bytes, 0x0b/0x0c/0x0e/0x10 = 4 bytes, 0x21 = 2 bytes. Unknown tags abort parsing. (H, jump table @0x41c3c8/0x41c3f4)
* **No "ice alarm" in the WH6006 alarm packet.** The string "Ice Alarm:" exists only in legacy dialog 135. (H)

---

## 6. Mobile network / GSM (dialog 159; save 0x41f520; fill 0x41f090; read 0x407170)

Reads (func 407170) are sent in this order: `11`, `13`, `07`, each with the rest zero. The replies are copied to obj+0x4230 / 0x4330 / 0x4430, which equal Mobile-dialog+0x148 / +0x248 / +0x348.
The save sends three writes in this order: `12`, `14`, `08`. **Pairing: 0x11↔0x12 (APN), 0x13↔0x14 (user/password), 0x07↔0x08 (SMS/phones).** The reply layouts equal the write layouts. (H: fill routine 41f18e-41f32b reads the same offsets)

### 6a. APN: read reply `0x11`, write `0x12`
| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x11 / 0x12 | 41f668 | H |
| 1 | 1 | u8 | **n + 2** (app quirk: the packet is n+3 bytes. Probably the device ignores p[1] and uses p[2]) | 41f675-41f67b | H (value), L (device behaviour) |
| 2 | 1 | u8 | APN length n | 41f681 | H |
| 3 | n | ASCII | APN | 41f6c9 | H |

### 6b. User / password: read reply `0x13`, write `0x14`
| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x13 / 0x14 | 41f6f1 | H |
| 1 | 1 | u8 | u + p + 4 (= total length) | 41f6e6-41f6eb | H |
| 2 | 1 | u8 | user length u | 41f6f8 | H |
| 3 | u | ASCII | user | 41f739 | H |
| 3+u | 1 | u8 | password length p | 41f74b | H |
| 4+u | p | ASCII | password | 41f78e | H |

### 6c. SMS settings + phone numbers: read reply `0x07`, write `0x08`
| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x07 / 0x08 | 41f8cf | H |
| 1 | 1 | u8 | t1+t2+t3+12 (= total length) | 41f8b5-41f8c3 | H |
| 2 | 2 | u16 LE | **SMS alarm interval**, seconds = minutes×60. UI 0..120 min ("Alarm Interval: 0 - 120 Minute"). Read shown ÷60 | 41f884-41f8f9 / 41f32b | H |
| 4 | 4 | u32 LE | **SMS "send current" interval**, seconds. UI unit combo 0x3f0: "Hour" (0..24 → ×3600) or "Minute" (0..59 → ×60). On read, < 3600 shows minutes, else hours | 41f793-41f855, 41f8ff-41f933 / 41f33e-41f3fb | H |
| 8 | 1 | u8 | **SMS ON/OFF**: 0=OFF, 1=ON (combo 0x3f1) | 41f936-41f948 / 41f244 | H |
| 9 | 1 | u8 | Tel1 length t1 (read side rejects > 17: "Tel length max 17") | 41f951 / 41f25d | H |
| 10 | t1 | ASCII | Tel1 | 41f989 | H |
| 10+t1 | 1 | u8 | Tel2 length t2 | 41f99a | H |
| 11+t1 | t2 | ASCII | Tel2 | 41f9d7 | H |
| 11+t1+t2 | 1 | u8 | Tel3 length t3 | 41f9ee | H |
| 12+t1+t2 | t3 | ASCII | Tel3 | 41fa2b | H |

* Blank interval fields give 0xFFFF / 0xFFFFFFFF. (H)
* Hidden combo 0x3ef ("Minute"/"Never") is not sent. (H)

### 6d. MSG test `0x8a` (func 0x41fc80)
1. The app first runs the full mobile save silently (12, 14, 08).
2. It then sends `8a 19 "Weather Logger Test MSG"`: p[1] = text_len+2, p[2..] = the ASCII text, a fixed string of 23 chars.
3. Expected reply 0x8a. The device also sends an unsolicited 0x8a, which the app shows as "Send Message Success.". (H)

### 6e. Sync network time `0x8e` (func 0x41fe00, button "Sync Net Time")
Request `8e` (rest 0). The text of the hidden edit 0x3fe is read but not sent. Expected/unsolicited reply 0x8e → "Sync Time Success.". (H)

---

## 7. Upload / web-server settings: read `0x09`/`0x0b`, write `0x0a`/`0x0c` (dialog 158; save 0x426080; fill 0x425280)

The "eight 0x88 sends" are really **four `0x0a` packets and four `0x0c` packets**. 0x88 is only the expected-reply byte. (H)

**Server sub-index** (protocol, sub 0..3) ↔ UI:

| sub | server | host the app forces on save |
|---|---|---|
| 0 | Wunderground | "rtupdate.wunderground.com" |
| 1 | Weathercloud | "api.weathercloud.net" |
| 2 | WOW (Met Office) | "wow.metoffice.gov.uk" |
| 3 | "New web" (custom) | user text |

The UI combo entry "www.ecowitt.net" maps to internal index 4 and is **never read or written**. The app builds a `0c 0d 04 00 …` packet for it at 4268f6 but never sends it. (H, 425650-426068 mapping, 4261ee-42621b)

Reads (func 406db0): `09 00`, `09 01`, `09 02`, `09 03`, then `0b 00`, `0b 01`, `0b 02`, `0b 03` (p[1] = sub). Replies are stored per sub at obj+0x5290+n·0x100 (0x09) and obj+0x5790+n·0x100 (0x0b). (H)

### 7a. Host: read reply `0x09`, write `0x0a`
| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x09 / 0x0a | 426264 | H |
| 1 | 1 | u8 | h + 4 (total) | 42626b-426278 | H |
| 2 | 1 | u8 | sub (0..3) | 42627e | H |
| 3 | 1 | u8 | host length h | 426289 | H |
| 4 | h | ASCII | host name | 4262d0 / read 4252c8 | H |

### 7b. Station credentials & options: read reply `0x0b`, write `0x0c`
| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x0b / 0x0c | 4262fd | H |
| 1 | 1 | u8 | i + p + 13 (total) | 4262e9-426304 | H |
| 2 | 1 | u8 | sub (0..3) | 42630a | H |
| 3 | 1 | u8 | **Server type** for custom server path: 0=jsp, 1=asp, 2=php (combo member 0xce8, labelled "Server type:", ID 0x3ef by position) | 4262e2/426311 / read 4253c8 | H (value), M (meaning) |
| 4 | 1 | u8 | **Upload enable**: bit0 0=Disable upload, 1=Enable upload (combo hwnd member 0xe44, probably ID 0x3f2; ID mapping M). The app sends the **same** global flag in all four subs. On read it keeps bit0 and stores the full byte as "upload type" | 4262dc / 4253db-425402 | H |
| 5 | 2 | u16 LE | **Port** (default "80"; edit 0x3f8) | 426317-426332 / 425408 | H |
| 7 | 4 | u32 LE | **Upload interval, seconds**. UI combo 0x3f1 is sorted ("Hour" idx0: 0..24 → ×3600; "Minute" idx1: 0..59 → ×60). Read: ≥ 3600 → hours. A single global value is sent in all subs | 4261a1-4261e8, 426338-426372 / 425420-425491 | H |
| 11 | 1 | u8 | Station ID length i | 426382 | H |
| 12 | i | ASCII | Station ID | 4263a5 / 425325 | H |
| 12+i | 1 | u8 | Password/key length p | 4263b0 | H |
| 13+i | p | ASCII | Password/key | 4263ef / 42537f | H |

* Send order: `0a sub0, 0c sub0, 0a sub1, 0c sub1, 0a sub2, 0c sub2, 0a sub3, 0c sub3`, each expecting reply 0x88. (H)
* "Upload type" Default/Customize (hidden combo 0x3f0) is **not** transmitted on write. (H)
* **The Enable/Disable dropdown belongs to the www.ecowitt.net entry.** Combo 0x3f2 (member 0xe44, items "Disable upload " / "Enable upload ") is shown (`ShowWindow(5)`) together with "Add To Ecowitt" (0x43e) only when the Web combo selects "www.ecowitt.net" (UI index 0, internal 4, the entry selected when the dialog opens). For Wunderground, Weathercloud, WOW and New web it is hidden and the Password edit 0x3f7 is shown in its place (server-select handler 425650: 4257be-4257f9 for ecowitt.net; 4259af, 425b8c-425ba1, 425d50, 425ef8-425f0d hide it for the others). The fill routine stores `byte4 & 1` from each 0x0b reply into the single field obj+0xbc4 (4253e6-4253fc), so the last server read wins, and the save copies obj+0xbc4 into byte 4 of all four 0x0c packets (4262dc). The app has **no per-server enable**. Whether the firmware treats the bit as the ecowitt.net switch or as a global upload switch is unknown. (H for the UI, L for the firmware meaning)
* There is **no 0x87 read**. The reads are 0x09/0x0b. (H)

---

## 8. Sensor IDs: read via `0x8f` sub 2, write `0x91` (dialog 206 "Sensors ID", 208 "ID Setting"; read 0x427560, write 0x428210)

### 8a. Read (func 427560)
The app reads 8 records. Each request is:

`8f 06 02 <addr lo> <addr hi> 14`, where addr = slot × 20 and count = 20 bytes.

The slots, in request order, are **0, 6, 14, 22, 31, 40, 3, 2**. Each 20-byte reply payload (`p[2..p[1])`, appended by the dispatcher into 0x44e6c0) is copied to the record table at 0x44eb78 + slot×20. (H)

20-byte sensor record:

| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | ? | unknown | - | - |
| 1 | 1 | u8 | signal level 0..4 (icon index; out of range → no icon) | 427906 | H (use), M (meaning) |
| 2 | 1 | u8 | battery indicator (0 or 0xFF → no icon, else icon) | 427a84 | M |
| 3 | 1 | ? | unknown | - | - |
| 4 | 4 | u32 LE | **Sensor ID**. `FFFFFFFF` = "Learning" (searching/re-register), `FFFFFFFE` = "Disable", else shown as `%X` | 427e93, 4277d0 | H |
| 8..19 | 12 | ? | unknown | - | - |

### 8b. Write `0x91`
| off | size | type | meaning | evidence | conf |
|---|---|---|---|---|---|
| 0 | 1 | u8 | 0x91 | 4282fd `WORD 0x0791` | H |
| 1 | 1 | u8 | 0x07 | 4282fd | H |
| 2 | 1 | u8 | slot (sensor type index, same numbers as the read) | 4282de | H |
| 3 | 4 | u32 LE | new ID (hex text parsed with `wcstoul(…,16)`, ≤ 8 hex digits). `FFFFFFFF` = re-register/learn | 4282bd-428309 | H |

* "Re-register" buttons send ID `FFFFFFFF` for slots 0, 6, 0x0e, 0x16, 0x1f, 0x28, 3, 2. After a write, the app re-reads all records. (H)
* The ID Setting dialog accepts a hex ID or "Learning". Choosing "Disable" makes the app show "Please enter ID" and send nothing, so writing `FFFFFFFE` is **not** exercised by the app (L that the device accepts it). (H)
* Slot ↔ sensor (UI bitmaps #205,#210,#200,#211,#204,#203,#201,#202). The numbering matches Ecowitt's GW1000 sensor-type indices: 0=WH65/WH24 outdoor array, 2=WH80 (ultrasonic), 3=WH40 rain, 6=WH31 CH1 (temp/humi), 14=WH51 CH1 (soil moisture), 22=WH41 CH1 (PM2.5), 31=WH34 CH1 (soil temp), 40=WH35 CH1 (leaf). (M)

---

## 9. Reset commands (Reset dialog 181; caller 0x408290; also 0x424840)

| action | UI | packet | expected reply | side effects in app | evidence | conf |
|---|---|---|---|---|---|---|
| Device reset (reboot) | "Device Reset" → "Are you sure to reset device?" → EndDialog(200) | `86` (rest 0) | 0x86 | none | 421330, 408619-408683 | H |
| Factory reset | "Factory Reset" → "Are you sure to reset factory setting?" → EndDialog(100) | `88` (rest 0, p[1]=0) | 0x88 | deletes local `*.csv` history first; sets obj+0x32cc=1 | 421360, 40833f-40859a | H |
| Reset from firmware-update dialog | "Are you sure to reset the device?" | `88` (rest 0) | 0x88 | deletes `*.csv` | 424840-424a96 | H |

The bare `88` is distinguished from web-server writes (0x0a/0x0c, which also expect reply 0x88) by its command byte. The mapping of Device Reset to 0x86 and Factory Reset to 0x88 comes from the confirmation strings on the two handlers. (H for bytes, M for which button)

---

## 10. Other configuration-related writes found

* **Clear max/min `0x10`** (func 0x4210c0, "Clear Success"). The packet is constant, 45 bytes:
  `10 2d 01 00 00 02 00 00 06 00 00 07 00 00 03 00 00 08 00 00 05 00 04 00 0b 00 0c 00 0e 00 0f 00 10 00 15 00 16 00 17 00 22 00 00 23 00`.
  It reads as a list of item tags, each followed by zero bytes. (H bytes, L interpretation)

---

## 11. Legacy brace-framed protocol (NOT the WH6006 command set; for completeness)

Some dialogs build frames of the form `7b … 40 7d` (`{ … @ }`). These are probably for an older logger model that shares this app.
* **CH1..CH8 calibration** (dialog 134, save 0x40e190):
  1. Send `7b 51 40 7d` (`{Q@}`).
  2. If the device is on its main screen, send `7b 11 [CH1..CH8: temp_hi temp_lo humi]×8 40 7d`: temp is s16 **big-endian** in 0.1 °C (0xFFFF = blank), humi is s8.
  3. Send `7b 2f 40 7d` (`{/@}`).
  The app reuses expected-reply codes 0x0c/0x13. (H bytes, M purpose)
* **Set interval** (0x42c800, "Interval Range of values: 5 to 240 Minute"): `7b 40 <minutes> 40 7d`. (H)
* The old alarm dialog 135 (with "Ice Alarm:") belongs to this family too. 0x40b080/0x42b0b0 were not decoded.

---

## Appendix A: TimeZone index table (settings p[7])
This is the app's combo order, which is also the table at 0x43ac98 (28-byte entries: +0 s16 UTC offset in minutes, +4 name ptr, +8.. Windows-style DST rule). The device presumably uses the same index list (M). Index 42 = UTC.

| idx | UTC offset (min) | DST rule in app table | Name |
|---|---|---|---|
| 0 | -720 | no | (UTC-12:00)International Date Line West |
| 1 | -660 | no | (UTC-11:00)Samoa |
| 2 | -660 | no | (UTC-11:00)Coordinated Universal Time-11 |
| 3 | -600 | yes | (UTC-10:00)Aleutian Islands |
| 4 | -600 | no | (UTC-10:00)Hawaii |
| 5 | -570 | no | (UTC-09:30)Marquesas Islands |
| 6 | -540 | yes | (UTC-09:00)Alaska |
| 7 | -540 | no | (UTC-09:00)Coordinated Universal Time-9 |
| 8 | -480 | yes | (UTC-08:00)Pacific Time (US & Canada) |
| 9 | -480 | yes | (UTC-08:00)Baja California |
| 10 | -480 | no | (UTC-08:00)Coordinated Universal Time-8 |
| 11 | -420 | yes | (UTC-07:00)Chihuahua,La Paz,Mazatlan |
| 12 | -420 | yes | (UTC-07:00)Mountain Time (US & Canada) |
| 13 | -420 | no | (UTC-07:00)Arizona |
| 14 | -360 | yes | (UTC-06:00)Guadalajara,Mexico City,Monterrey |
| 15 | -360 | no | (UTC-06:00)Saskatchewan |
| 16 | -360 | yes | (UTC-06:00)Central Time (US & Canada) |
| 17 | -360 | no | (UTC-06:00)Central America |
| 18 | -300 | no | (UTC-05:00)Bogota,Lima,Quito |
| 19 | -300 | yes | (UTC-05:00)Eastern Time (US & Canada) |
| 20 | -300 | yes | (UTC-05:00)Havana |
| 21 | -300 | yes | (UTC-05:00)Hayti |
| 22 | -300 | no | (UTC-05:00)Chetumal |
| 23 | -300 | yes | (UTC-05:00)Indiana (East) |
| 24 | -270 | no | (UTC-04:30)Caracas |
| 25 | -240 | yes | (UTC-04:00)Atlantic Time (Canada) |
| 26 | -240 | yes | (UTC-04:00)Cuiaba |
| 27 | -240 | no | (UTC-04:00)Georgetown,La Paz,Manaus,San Juan |
| 28 | -240 | yes | (UTC-04:00)Santiago |
| 29 | -240 | yes | (UTC-04:00)Asuncion |
| 30 | -210 | yes | (UTC-03:30)Newfoundland |
| 31 | -180 | yes | (UTC-03:00)Brasilia |
| 32 | -180 | no | (UTC-03:00)Buenos Aires |
| 33 | -180 | yes | (UTC-03:00)Greenland |
| 34 | -180 | no | (UTC-03:00)Cayenne,Fortaleza |
| 35 | -180 | no | (UTC-03:00)Montevideo |
| 36 | -120 | no | (UTC-02:00)Coordinated Universal Time-02 |
| 37 | -60 | no | (UTC-01:00)Cape Verde Is. |
| 38 | -60 | yes | (UTC-01:00)Azores |
| 39 | +0 | yes | (UTC)Dublin,Edinburgh,Lisbon,London |
| 40 | +0 | yes | (UTC)Casablanca |
| 41 | +0 | no | (UTC)Monrovia,Reykjavik |
| 42 | +0 | no | (UTC)Coordinated Universal Time |
| 43 | +60 | yes | (UTC+01:00)Amsterdam,Berlin,Bern,Rome,Stockholm,Vienna |
| 44 | +60 | yes | (UTC+01:00)Belgrade,Bratislava,Budapest,Ljubljana,Prague |
| 45 | +60 | yes | (UTC+01:00)Brussels,Copenhagen,Madrid,Paris |
| 46 | +60 | yes | (UTC+01:00)Sarajevo,Skopje,Warsaw,Zagreb |
| 47 | +60 | yes | (UTC+01:00)Windhoek |
| 48 | +60 | no | (UTC+01:00)West Central Africa |
| 49 | +120 | yes | (UTC+02:00)Amman |
| 50 | +120 | yes | (UTC+02:00)Beirut |
| 51 | +120 | yes | (UTC+02:00)Damascus |
| 52 | +120 | no | (UTC+02:00)Harare,Pretoria |
| 53 | +120 | yes | (UTC+02:00)Helsinki,Kyiv,Riga,Sofia,Talinn,Vilnius |
| 54 | +120 | no | (UTC+02:00)Cairo |
| 55 | +120 | yes | (UTC+02:00)Athens,Bucharest,Istanbul |
| 56 | +120 | yes | (UTC+02:00)Jerusalem |
| 57 | +180 | no | (UTC+03:00)Baghdad |
| 58 | +180 | no | (UTC+03:00)Kuwait,Riyadh |
| 59 | +180 | no | (UTC+03:00)Minsk |
| 60 | +180 | no | (UTC+03:00)Moscow,St.Petersburg,Volgograd |
| 61 | +180 | no | (UTC+03:00)Nairobi |
| 62 | +210 | yes | (UTC+03:30)Tehran |
| 63 | +240 | no | (UTC+04:00)Abu Dhabi,Muscat |
| 64 | +240 | no | (UTC+04:00)Yerevan |
| 65 | +240 | no | (UTC+04:00)Baku |
| 66 | +240 | no | (UTC+04:00)Tbilisi |
| 67 | +240 | no | (UTC+04:00)Port Louis |
| 68 | +270 | no | (UTC+04:30)Kabul |
| 69 | +300 | no | (UTC+05:00)Tashkent |
| 70 | +300 | no | (UTC+05:00)Ekaterinburg |
| 71 | +300 | no | (UTC+05:00)Islamabad,Karachi |
| 72 | +330 | no | (UTC+05:30)Chennai,Kolkata,Mumbai,New Delhi |
| 73 | +330 | no | (UTC+05:30)Sri Jayawardenepura |
| 74 | +345 | no | (UTC+05:45)Kathmandu |
| 75 | +360 | no | (UTC+06:00)Astana |
| 76 | +360 | no | (UTC+06:00)Dhaka |
| 77 | +360 | no | (UTC+06:00)Novosibirsk |
| 78 | +390 | no | (UTC+06:30)Yangon (Rangoon) |
| 79 | +420 | no | (UTC+07:00)Kobdo |
| 80 | +420 | no | (UTC+07:00)Krasnoyarsk |
| 81 | +420 | no | (UTC+07:00)Bangkok,Hanoi,Jakarta |
| 82 | +480 | no | (UTC+08:00)Beijing,Chongqing,Hong Kong,Urumqi |
| 83 | +480 | no | (UTC+08:00)Kuala Lumpur,Singapore |
| 84 | +480 | no | (UTC+08:00)Perth |
| 85 | +480 | no | (UTC+08:00)Taipei |
| 86 | +480 | no | (UTC+08:00)Ulaanbaatar |
| 87 | +480 | no | (UTC+08:00)Irkutsk |
| 88 | +540 | no | (UTC+09:00)Pyongyang |
| 89 | +540 | no | (UTC+09:00)Osaka,Sapporo,Tokyo |
| 90 | +540 | no | (UTC+09:00)Seoul |
| 91 | +540 | no | (UTC+09:00)Yakutsk |
| 92 | +570 | yes | (UTC+09:30)Adelaide |
| 93 | +570 | no | (UTC+09:30)Darwin |
| 94 | +600 | no | (UTC+10:00)Brisbane |
| 95 | +600 | no | (UTC+10:00)Vladivostok |
| 96 | +600 | no | (UTC+10:00)Guam,Port Moresby |
| 97 | +600 | yes | (UTC+10:00)Hobart |
| 98 | +600 | yes | (UTC+10:00)Canberra,Melbourne,Sydney |
| 99 | +630 | yes | (UTC+10:30)Lord Howe Island |
| 100 | +660 | no | (UTC+11:00)Magadan |
| 101 | +660 | no | (UTC+11:00)Solomon Is.,New Caledonia |
| 102 | +720 | yes | (UTC+12:00)Auckland,Wellington |
| 103 | +720 | yes | (UTC+12:00)Fiji |
| 104 | +720 | no | (UTC+12:00)Coordinated Universal Time+12 |
| 105 | +765 | yes | (UTC+12:45)Chatham Islands |
| 106 | +780 | no | (UTC+13:00)Nuku'alofa |
| 107 | +840 | no | (UTC+14:00)Christmas Island |

## Appendix B: Quick reference (request bytes)
| purpose | request | reply/ack |
|---|---|---|
| read settings | `05` | 0x05, layout §3 |
| write settings | `06 (12+n) …` | 0x06 |
| sync time (UTC) | `02 0a 18 YYhi YYlo MM DD hh mm ss` | 0x02 |
| set rain totals | `02 19 10 dd dd 11 w w w w 12 m m m m 13 y y y y 14 t t t t` | 0x02 |
| read calibration | `03` | 0x03, §4 |
| write calibration | `04 19 01 t 02 t 06 h 07 h 08 pp 09 pp 0a dd 0b g 0d g 15 g` | 0x04 |
| read alarms | `0d` | 0x0d, §5 |
| write alarms | `0e 31` or `0e 33 …` | 0x0e |
| read APN / user+pw / SMS+tel | `11` / `13` / `07` | same cmd |
| write APN / user+pw / SMS+tel | `12 …` / `14 …` / `08 …` | same cmd |
| MSG test | `8a 19 "Weather Logger Test MSG"` | 0x8a |
| sync net time | `8e` | 0x8e |
| read web host / creds | `09 <sub>` / `0b <sub>`, sub 0..3 | 0x09 / 0x0b |
| write web host / creds | `0a …` / `0c …` | 0x88 |
| read sensor record | `8f 06 02 <slot*20 LE16> 14` | 0x8f |
| write sensor ID | `91 07 <slot> <id LE32>` | 0x91 |
| device reset | `86` | 0x86 |
| factory reset | `88` | 0x88 |
| clear max/min | `10 2d …` (constant, §10) | 0x10 |
| device name | `19` | 0x19 |
