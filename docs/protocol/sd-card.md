# WH6006 "Weather Logger" v1.7.3: SD card, history and DB spec

All of this comes from static analysis of `Weather Logger.exe` (`$S/ann.txt`). Addresses are VAs in that exe.
Conventions: `p[i]` = byte i of the 64-byte HID payload (report id stripped). The rx buffer is at 0x446950, so `p[i]` = byte at 0x446950+i.
Dispatcher struct `D` = 0x44e678. `D+0x20` = 0x44e698 holds the expected reply cmd, `D+0x28` = 0x44e6a0 is the 0x8f append offset, `D+0x44` = 0x44e6bc is the 0x16 chunk counter, `D+0x48` = 0x44e6c0 is the 0x8f append buffer, and `D+0x148` = 0x44e7c0 is a copy of the 0x18 reply.
SD dialog object = `M+0x12f8` (M is the main object the dispatcher gets at `[esp+0x14]`).
Confidence: **H** = read directly from the code; **M** = inferred with good evidence; **L** = a guess, flagged as such.

---

## 0. Key findings

1. **History is NOT read from device memory via 0x8f.** In this app, history comes only from **CSV files on the logger's SD card**. The app lists them with cmd 0x15, downloads one with 0x16, and deletes one with 0x17. It then parses the CSV locally and loads it into `Records` in the MDB. (H)
2. **0x8f is a raw memory read for something else.** Sub 1 reads the 244-byte live/status block. Sub 2 reads 20-byte sensor-slot blocks. See §5. No history decoding happens in 4063d0/4064e0/427560. (H)
3. **There is no CSV *export* in this app.** 408290 and 424840 are the device-reset handlers. They delete the local `*.csv` cache and send `0x88` or `0x86`. The CSV files are produced by the device. (H)
4. **The MDB password is empty.** The connection string is `Provider=Microsoft.Jet.OLEDB.4.0;Data Source=%s;Persist Security Info=False;Jet OLEDB:Database Password=%s`, and the second `%s` is an empty wide string: `[ebp-0x234]` is set to 0 at 40ec08, then `swprintf_s` runs at 40ec0f. `wh60003g4g` is **not** the password; it is the single-instance mutex name (`CreateMutexW` at 401667). (H)

---

## 1. Cmd 0x15: SD file listing (func 416fa0, parser 4171b0, dispatcher case 0x42941e)

### Request (H, 416fb2..416ffb)
```
p[0]=0x15, p[1..63]=0
```
Before sending, the app sets expected cmd = 0x15. It then waits up to 2000 ms (0x7d0) on the reply event.

### Reply stream (H)
The device answers with **one 0x15 packet per file**, then an **end marker**.

| off | size | type | meaning | evidence |
|---|---|---|---|---|
| 0 | 1 | u8 | 0x15 | dispatcher compares `dl` (=p[0]) with 0x15 at 42941e |
| 1 | 1 | u8 | not read by the app (probably packet length) | (not referenced) |
| 2 | 4 | u32 LE | file size in bytes | 417265..41729a (`dword [rx+2]` reassembled LE) |
| 6 | 1 | u8 | name length N; **0 means end of list** | 42944b `cmp byte [0x446956],0` → 0x429202 |
| 7 | N | ASCII/ANSI | file name, no NUL needed | 417219..41722c `memcpy(buf, rx+7, rx[6])` into a zeroed 128-byte buffer |

- When p[6] != 0, the dispatcher calls `4171b0(rx)` (ebx = SD dialog) and increments the file count `dlg+0x105c` (M+0x2354, 429470). It does **not** signal the event, so more packets are expected.
- When p[6] == 0, the dispatcher clears the expected cmd and SetEvents (429202). That ends the listing.
- If a packet whose p[0] != 0x15 arrives while 0x15 is expected, `dlg+0x1478` = 0 (429427). The app then shows "Read SDCard fail,please reopen the menu."
- Storage: names go to `CString dlg[0xb8 + 4*i]` and size strings to `dlg[0x888 + 4*i]`, so at most **500 entries** (0x7d0/4) (M). A map name→size (`dlg+0x1430`, 408910) is used later by the download.
- Size display: size ≤ 0x100000 → `"%0.2f KB"` (size × 1/1024). Otherwise `"%0.2f MB"` (size × 1/1024 × 1/1024). The constant at 0x43f4e0 is the double 0.0009765625. (H)
- List columns are "File Name" and "Size" (416e1f/416e3a).

### Follow-up: SD status via cmd 0x18 (H, 417039..41709c)
Right after the list, 416fa0 sends `p[0]=0x18`, rest 0, expects 0x18, and waits 2000 ms. The dispatcher's 0x18 case (42977a) copies the 64-byte reply to `D+0x148` = 0x44e7c0. **SD status = reply byte p[20]** (0x44e7d4 = 0x44e7c0+0x14):

| p[20] | text |
|---|---|
| 0 | "SD Initializing" |
| 1 | "Normal" |
| 2 | "No SD Card" |
| 3 | "Mount Failed" |
| other | "Open Failed" |

If the 0x18 wait times out (0x102), or the listing was flagged bad, the app shows "Read SDCard fail,please reopen the menu.". Other bytes of the 0x18 reply are outside this area.

---

## 2. Cmd 0x16: SD file download (func 4173a0, dispatcher case 0x429486, watchdog thread 419a10)

### Local file (H)
- Path: `C:\Users\Public\WeatherLogger\<devicefilename>` (`CWinApp+0xb8` is set to that dir at 4019ad; it is joined with `"\%s"` at 4174f6). No extension is added. The reset handlers delete `C:\Users\Public\WeatherLogger\*.csv`, which implies device names end in `.csv` (M).
- The file is opened as an MFC `CStdioFile` in **text mode**: Open(path, modeReadWrite=2), or if it is missing, Open(path, modeCreate|modeReadWrite=0x1002) (4175d1 / 417659). Errors: "Open Error1" / "Create Error3".

### Resume offset computation (H, 4176be..41785b)
1. `local_size` = CFile::GetStatus(path).m_size (`[ebp-0x764]`).
2. `lines` = number of `ReadString` successes over the whole local file (loop 417737..417768).
3. `offset = local_size - lines`. This undoes the CRLF expansion that text mode applied on write, so the device file is assumed to use LF line endings (M).
4. `dev_size` = size from the 0x15 map, looked up by name (4089f0). If the name is not in the map → " m_FilenameSize Error.".
5. If the file already existed and `dev_size - offset < -1`, the local file is larger than the device file. The app then closes, deletes and recreates it (error "Create Error4"), and sets `offset = 0`.
6. `remaining = dev_size - offset`. The progress bar range is set to 0..remaining (PBM_SETRANGE32 0x406). The file is seeked to end (4090c0 = Seek(0, end)), so data is **appended**.

### Request (H, 41792d..4179fe)
Name = the device file name converted to ANSI (CP_THREAD_ACP via 409090). N = its length.
```
p[0]  = 0x16
p[1]  = N + 11              total length (cmd+len+4+4+1+N)
p[2..5]  = offset   u32 LE  (bytes already held locally, in device bytes)
p[6..9]  = remaining u32 LE (dev_size - offset)
p[10] = N
p[11..11+N) = file name bytes (no NUL)
rest = 0
```
Before sending: wait 3000 ms on the event, ResetEvent, set `0x44e6bc = 0` and expected = 0x16. After sending, the app starts the watchdog thread (AfxBeginThread 419a10) and waits **INFINITE** on the event.

### Reply chunks (H, 429486..429632)
| off | size | type | meaning |
|---|---|---|---|
| 0 | 1 | u8 | 0x16 |
| 1 | 1 | u8 | not read |
| 2 | 1 | u8 | L = data length in this chunk |
| 3 | L | bytes | file data (text) |

- Every chunk increments `0x44e6bc` (watchdog heartbeat).
- **L ≥ 0x3d (61): intermediate chunk.** The app copies L bytes into a zeroed buffer, converts them as a NUL-terminated ANSI string (MultiByteToWideChar CP 3, 428d10), and calls `CStdioFile::WriteString` (vtable+0x60). It adds L to the total (`dlg+0x1058`), updates the progress bar, and does **not** signal.
- **L < 61: last chunk.** The app writes it the same way and shows "ok..." (control 0x43d). It then sets progress, **Closes** the file (vtable+0x58), sets expected = 0 and `0x44e6bc = -1`, and SetEvents.
- So a full chunk carries 61 data bytes. The stream ends with the first chunk shorter than 61 bytes. If `remaining` is an exact multiple of 61, the device must presumably send a zero-length final chunk; device behaviour is unknown (L).
- Because data goes through `%s`/WriteString, **NUL bytes would truncate the chunk**. The file is text: CSV (H for "text"; M for "CSV", based on the parser in §3).

### Watchdog / state machine (H, 419a10)
```
IDLE --send 0x16 req--> RECEIVING (counter=0, expected=0x16)
RECEIVING --0x16 chunk L>=61--> RECEIVING (append, counter++)
RECEIVING --0x16 chunk L<61--> DONE (append, close, counter=-1, signal)
RECEIVING --counter unchanged for >50 polls of 100 ms (~5 s)--> TIMEOUT (counter=-100)
after wait: if counter==-100 or wait==WAIT_TIMEOUT → close file, "Read file errors.", abort
```
Note: the watchdog does not SetEvent, so in the original app a stall can hang the INFINITE wait. A reimplementation should use a ~5 s inactivity timeout (M).

### After a successful download (H, 417b42..418f49)
The app reopens the local file read-only and parses it as CSV (§3). It **clears** the DB first (40ebb0 opens the MDB; 40f5c0 runs `DELETE * FROM Records` and `DELETE * FROM Param`), then loads the rows into `Records`. The DB therefore only ever holds the most recently downloaded file.

---

## 3. CSV file format as parsed by the app (4173a0, 417cd6..418cad)

- Line 1 is the **header**. Its field count = number of `,` plus 1 (417350, 417d18), at most 30 fields. Each header field becomes a list-view column (width 0x50).
- **Unit extraction** (H): for header fields 1, 8, 13, 10 and 21, the app takes the substring starting at `(` (CString::Find("(") then Mid). Field 1's unit has `C`→`°C` and `F`→`°F` replaced. These give: temperature unit (f1) → dlg+0x1444, pressure unit (f8) → +0x1448, rain unit (f13) → +0x144c, wind unit (f10) → +0x1450, light unit (f21) → +0x1454. So header fields look like `Name(unit)` (M).
- **Data lines** are split on `,` using the header's field count.
  - Each raw field is shown in the grid as text.
  - Fields are converted into a 0x78-byte record (below), which is written to a temp file (`GetTempFileName(%TEMP%, "WH60")`) and then inserted with ADO AddNew/Update (40f410 → 40f720).
- **Missing value**: if a field *contains* `--`, float fields become 65535.0 (float at 0x43f430) and int fields become 0xFFFF (65535). DB queries filter `<>65535`. (H)
- **Time** (f0) is parsed with `VarDateFromStr(str, LCID 0, LOCALE_NOUSEROVERRIDE 0x400)`, which is locale-dependent. On failure it becomes 0.0 if the HRESULT is DISP_E_TYPEMISMATCH, else -1.0. It is stored as an OLE DATE (double). (H)
- The record buffer is zeroed **once** before the loop (417c55). Fields not parsed from CSV therefore stay 0 for every row.

### CSV column → record → DB mapping (H for index→offset→column; column *names* in CSV are not in the exe)

| CSV idx | parse | record off | type | DB column | evidence (store) |
|---|---|---|---|---|---|
| 0 | VarDateFromStr | 0x00 | double (DATE) | Time | 41842d |
| 1 | _wtof | 0x18 | f32 | InTemp | 4184ab |
| 2 | _wtof → ftol | 0x1c | i32 | InHumi | 41851b |
| 3 | _wtof | 0x20 | f32 | OutTemp | 4185a0 |
| 4 | _wtoi | 0x24 | i32 | OutHumi | 418603 |
| 5 | _wtof | 0x10 | f32 | DewPoint | 41885b |
| 6 | _wtof | 0x3c | f32 | WindChill | 4188d3 |
| 7 | _wtof | 0x14 | f32 | — (not stored in DB; unknown, maybe heat index) **L** | 41894b |
| 8 | _wtof | 0x2c | f32 | AbsPressure | 4186f3 |
| 9 | _wtof | 0x28 | f32 | RelPressure | 41867b |
| 10 | _wtof | 0x30 | f32 | Wind | 41876b |
| 11 | _wtof | 0x34 | f32 | Gust | 4187e3 |
| 12 | not parsed | — | — | (WindDirection column stays 0) **M: likely wind direction text** | — |
| 13 | not parsed (header gives rain unit) | — | — | — | 417ec0 |
| 14 | not parsed | — | — | — | — |
| 15 | _wtof | 0x40 | f32 | RainHour | 4189c3 |
| 16 | _wtof | 0x44 | f32 | RainDay | 418a3b |
| 17 | _wtof | 0x48 | f32 | RainWeek | 418ab3 |
| 18 | _wtof | 0x4c | f32 | RainMonth | 418b2b |
| 19 | _wtof | 0x50 | f32 | RainYear | 418ba3 |
| 20 | not parsed | — | — | — | — |
| 21 | _wtof | 0x58 | f32 | Light | 418c1b |
| 22 | _wtoi | 0x60 | i32 | UV | 418c77 |

Pairing note: in each block the `fstp [ebp-0x12xx]` that follows `lea …,[ebp-0x4yy]` stores the value of the **previous** field. The table applies that rule. Field i lives at `ebp-0x4d8+4*i`, and record base = `ebp-0x1244`.

### Record struct (0x78 bytes) → ADO field writer 40f720 (H)
| off | type | DB column (VT used) |
|---|---|---|
| 0x00 | double | Time (VT_DATE) |
| 0x08 | 8 bytes | unused here |
| 0x10 | f32 | DewPoint |
| 0x14 | f32 | (not written to DB) |
| 0x18 | f32 | InTemp (VT_R4) |
| 0x1c | i32 | InHumi (VT_INT) |
| 0x20 | f32 | OutTemp |
| 0x24 | i32 | OutHumi |
| 0x28 | f32 | RelPressure |
| 0x2c | f32 | AbsPressure |
| 0x30 | f32 | Wind |
| 0x34 | f32 | Gust |
| 0x38 | i32 | WindDirection |
| 0x3c | f32 | WindChill |
| 0x40 | f32 | RainHour |
| 0x44 | f32 | RainDay |
| 0x48 | f32 | RainWeek |
| 0x4c | f32 | RainMonth |
| 0x50 | f32 | RainYear |
| 0x54 | f32 | Rain (never set from CSV → 0) |
| 0x58 | f32 | Light |
| 0x5c | 4 bytes | unused |
| 0x60 | i32 | UV |
| 0x64 | i32 | No (never set from CSV → 0) |
| 0x68..0x77 | — | unused |

The CSV path applies no scaling: values are stored exactly as written in the CSV, in the units named in the header.

---

## 4. Cmd 0x17: SD delete file (func 4196e0)

### Request (H, 41988b..4198f0)
```
p[0] = 0x17
p[1] = N + 3
p[2] = N
p[3..3+N) = file name (ANSI, no NUL)
rest = 0
```
The app asks "Are you sure to delete the file?" first. It sets expected = 0x17 and waits **500 ms** (0x1f4). The reply is not inspected: 0x17 falls into the dispatcher's default case (signal only). It then deletes the local copy (`DeleteFileW`), removes the list item, and reopens/clears the DB (40ebb0 + 40f5c0). (H)

---

## 5. Cmd 0x8f: raw memory read (NOT history)

Request format (H): `p[0]=0x8f, p[1]=0x06, p[2]=sub, p[3]=addr lo, p[4]=addr hi, p[5]=count`.
Reply (H, dispatcher 429064): the app appends `p[1]-2` data bytes from `p[2]` to `D+0x48` (0x44e6c0) at offset `D+0x28`. It adds `p[1]-2` to both that offset and `obj+0x3610` (M+0x36d8). So the reply's `p[1]` = 2 + data length.

### Sub 1: live/status block (4063d0)
- `obj+0x3610` = 0 and the append offset = 0. The app then loops while `obj+0x3610 < 0xf4`, sending `8f 06 01 <addr lo> <addr hi> 3c`, where addr = bytes received so far. That gives addr = 0, 60, 120, 180, 240 if each reply returns 60 bytes. Each read waits 2000 ms, and a timeout aborts the loop. (H)
- When complete, the app copies 244 bytes (0x3d dwords) from 0x44e6c0 to 0x44ef38 and decodes them in 4066a0 (H). These fields belong to the live-data area, not to history. Fields seen, with offsets into the 244-byte block:
  - +0x00 u32 Unix time (gmtime → "%d-%02d-%02d" and "%02d:%02d:%02d")
  - +0x08 u16 sensor-learning countdown in seconds (title "Learning Sensors ... %d s")
  - +0x0a u8 0..100 "%d %%" (an icon is picked per 10 %)
  - +0x0c u8 0xFF = "NO SIMCARD"
  - +0x0d u8 network 1..4 = 2G/3G/4G/5G
  - +0x0e u8 0..5 signal level
  - Many i16/u16/u32 values at +0x10..+0x2c, +0x34, +0x44, +0x64, +0x74, +0x90, +0xc0..+0xda are copied to globals 0x44e8d0..0x44e974 for the live-data owner to decode.

### Sub 2: sensor-slot blocks (4064e0 at startup, 427560 after the 0x91 sensor-ID save)
- For slot index k in **{0,2,3,6,14,22,31,40}** (4064e0) or **{0,6,14,22,31,40,3,2}** (427560): the app sends `8f 06 02 <(k*20)&0xff> <(k*20)>>8> 14`. That reads 20 bytes at address k*20, with the append offset reset to 0 each time. The 20 bytes are stored at `0x44eb78 + k*20`. (H)
- Byte +1 of each block (a value 0..4) feeds per-sensor status icons (4278d0) and globals 0x44e915..0x44e91c (H). This belongs to the sensor-ID area.

---

## 6. MDB `WeatherLoggerGSM.mdb` (Jet 4, page size 4096, **password = empty**)

Parsed with `$S/scripts/mdbschema.py` (stdlib-only Jet4 TDEF reader). User tables:

**Records** (TDEF page 50; column numbers have a gap at 17, from a deleted column)
| col# | name | Jet type |
|---|---|---|
| 0 | No | Long (INT32) |
| 1 | Time | DateTime |
| 2 | Interval | Long (never written by app) |
| 3 | InTemp | Single (FLOAT32) |
| 4 | InHumi | Long |
| 5 | OutTemp | Single |
| 6 | OutHumi | Long |
| 7 | Wind | Single |
| 8 | Gust | Single |
| 9 | WindDirection | Long |
| 10 | DewPoint | Single |
| 11 | WindChill | Single |
| 12 | RelPressure | Single |
| 13 | AbsPressure | Single |
| 14 | Light | Single |
| 15 | UV | Single (app writes VT_INT) |
| 16 | Rain | Single |
| 18 | RainWeek | Single |
| 19 | RainMonth | Single |
| 20 | RainYear | Single |
| 21 | RainTotals | Single (never written by app) |
| 22 | RainHour | Single |
| 23 | RainDay | Single |

**Param** (TDEF page 47): RecordTime DateTime, RainCounter Long, m_nPage Long, m_nPage_Record Long, InTempUnit Long, PressureUnit Long, RainUnit Long, WindUnit Long, LightUnit Long. **This version never writes it**: there are no references to these column names in the exe, only `DELETE * FROM Param`. It is legacy from the older memory-page history protocol (L).

App SQL (H): `SELECT * FROM Records`, `SELECT * FROM Records where Time>#%s# and Time<=#%s# and %s<>65535 ORDER BY [Time] ASC`, `SELECT Max(%s) as MAX1,Min(%s) as Min1 FROM Records where …`, `SELECT count(*) as [Count] FROM Records where …`, `DELETE * FROM Records`, `DELETE * FROM Param`.

The DB is copied from the install dir to `C:\Users\Public\WeatherLogger\WeatherLoggerGSM.mdb` at startup (CopyFileW at 4019db, fail-if-exists).

---

## 7. Related (reset handlers that touch the SD cache)
- 424840 "Are you sure to reset the device?": deletes `C:\Users\Public\WeatherLogger\*.csv`, then sends `[0x88]` (rest 0), waits, and shows "Success". (H)
- 408290 (dialog 0xb5 "Device Reset" / "Factory Reset"): result 100 → delete local `*.csv` and send `[0x88]`; result 200 → send `[0x86]`. Which button gives which result was not resolved (M).
- 419c80: opens the data folder in Explorer (ShellExecute "open").
