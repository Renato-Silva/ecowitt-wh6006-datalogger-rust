# WH6006 "Weather Logger" 1.7.3: version query, firmware update / bootloader, online update check, device open

This comes from static disassembly only ($S/ann.txt). Each item has a confidence level: **H** = read directly from code, **M** = strong inference, **L** = guess or analogy.
Packet notation: `payload[i]` is the 64-byte HID payload. On the wire, report ID 0x00 comes first, giving 65 bytes.

---------------------------------------------------------------------------------------------------
## 1. Device enumeration / open (func 0x4298d0, send 0x429b00)  [H]

- `HidD_GetHidGuid` → `SetupDiGetClassDevsW(guid, 0, 0, DIGCF_PRESENT|DIGCF_DEVICEINTERFACE (0x12))` → enumerate interfaces → `SetupDiGetDeviceInterfaceDetailW`.
- For each interface, it opens a probe handle with `CreateFileW(path, access=0, share=3, NULL, OPEN_EXISTING, 0x80, 0)`, calls `HidD_GetAttributes`, then closes the handle.
- Match: **VendorID == 0x04D8 && ProductID == 0x003C**. No other VID/PID compare exists anywhere in the exe. The only call sites are startup (0x403447) and the WM_DEVICECHANGE handler (0x40362a). [H]
- On a match it opens **two separate handles**: [H]
  - read handle (global 0x4464e0): `CreateFileW(path, GENERIC_READ (0x80000000), share 3, NULL, OPEN_EXISTING, FILE_FLAG_OVERLAPPED|FILE_ATTRIBUTE_NORMAL (0x40000080), 0)`
  - write handle (global 0x4464e4): the same call but with `GENERIC_WRITE (0x40000000)`.
- Writes: `WriteFile(hWrite, 65-byte buf (buf[0]=0 report ID, buf[1..64]=payload), 0x41, NULL, &ovl)`, then **Sleep(5)**. The app never waits for overlapped completion; ERROR_IO_PENDING (0x3e5) counts as OK. [H]
- Reads: a dedicated thread (0x428dd0) runs an overlapped `ReadFile` loop of 65 bytes and strips byte 0. [H]
- Hot-plug: `RegisterDeviceNotificationW` with the HID GUID (DBT_DEVTYP_DEVICEINTERFACE=5). On DBT_DEVICEARRIVAL (0x8000) while disconnected, or on DBT_DEVICEREMOVECOMPLETE (0x8004) while connected, the app rescans for 04D8:003C (0x4035d0). [H]
- **Bootloader PID:** the app recognises only 04D8:003C. If the device re-enumerates when it enters bootloader mode (cmd 0x86), the bootloader must present the same 04D8:003C, or the PC app could not reconnect. [M] 04D8:003C is also Microchip's stock "HID Bootloader" VID/PID. [L, analogy]
  For Linux: after sending 0x86, expect a possible USB disconnect/reconnect and reopen 04D8:003C (hidraw). The app's retry window is 1.5 s plus 6×1 s.

---------------------------------------------------------------------------------------------------
## 2. Command table for this area

| cmd | Name (from strings or role) | Request | Reply expected? | Conf |
|---|---|---|---|---|
| 0x80 | CMD_SOFT_VER (version query) | `80 00 00 …` (all zero after cmd) | yes, 0x80 | H |
| 0x82 | CMD_ERASE_DEVICE | `82 00 00 …` | yes, 0x82 (10 s timeout) | H |
| 0x83 | program/write chunk | see §5.4 | **no** (no wait, 5 ms pacing only) | H |
| 0x84 | end of programming ("program complete") | `84 00 …` | no | H (bytes) / M (meaning) |
| 0x85 | read-back / verify chunk | see §5.6 | yes, 0x85 (1 s timeout) | H |
| 0x86 | enter bootloader (sent while app firmware runs) | `86 00 …` | no | H (bytes) / M (meaning) |
| 0x87 | leave bootloader / run application (reset) | `87 00 …` | no | H (bytes) / M (meaning) |

Command names: only "CMD_SOFT_VER" (the timeout string in the 0x80 loop) and "CMD_ERASE_DEVICE" (the timeout string in the 0x82 loop) appear in the binary. The other roles are inferred from context. The 0x82–0x87 set mirrors Microchip's HID bootloader: ERASE_DEVICE, PROGRAM_DEVICE, PROGRAM_COMPLETE, GET_DATA, RESET_DEVICE, plus 0x80 and 0x86 added. [L, analogy]

Note: the receive thread routes a reply by the **expected** command (global 0x44e698), not by rx[0]. Only 0x89, 0x8a and 0x8e are recognised by rx[0]. When expected = 0 (as set for 0x83, 0x84 and 0x87), nothing is stored and no event is signalled. [H]

---------------------------------------------------------------------------------------------------
## 3. 0x80 version query (CMD_SOFT_VER)

### Request [H]
`payload = 80 00 00 … 00`. Expected reply cmd = 0x80. Timeout 1000 ms (0x3e8).

### Reply layout [H]
The dispatcher copies the whole 64-byte packet (M+0x6170 = firmware dialog+0xC8, and M+0x7944 = main obj+0x787C).
- `payload[0]` = 0x80 (not checked)
- `payload[2]` = **length N** of an ASCII string
- `payload[3 .. 3+N)` = ASCII string, **no terminator needed**. payload[1] is not used by this parser.

String format (from parser 0x4076e0): `"<label>:<maj>.<min>.<patch>"` [H]
- label = text before the first ':'. Compared exactly (case-sensitive) with:
  - `"Firmware Ver"` → normal application firmware is running
  - `"Bootloader Ver"` → device is in bootloader mode
- version = text after ':', e.g. "1.2.3". It is split at the first two '.' and each part goes through `_wtoi` (major/minor/patch kept separately). It is shown as `"<label> : <version>"`.
- For the online compare, all '.' characters are removed and the remainder is passed to `_wtoi` (e.g. "1.0.5" → 105). See §6.
- Example: `80 ?? 12 'F','i','r','m','w','a','r','e',' ','V','e','r',':','1','.','0','.','5'` (N=0x12=18). The value of payload[1] is unknown and not used. [M for the example]

The firmware-update path (0x423970, state 0) uses only `strstr`-like logic: it copies `payload[2]` bytes from `payload[3]` (into a 31-byte local buffer) and checks that the text **contains "Bootloader"**. [H]

### Poll behaviour (thread 0x405d40) [H]
While connected and not busy, the app sends 0x80 about **every 1 s**. This is the heartbeat and mode detector.
- On the first "Firmware Ver" reply after connecting, the app reads the settings (0x05, 0x0d, 0x03, 0x11/0x13/0x07, 0x0f) and starts the live polls (0x8f, 0x18).
- On "Firmware Ver", the version is saved to Main.ini `[FirmwareVer] Ver=<x.y.z>`, and the online firmware check thread starts (once per connect, gated on main+0x32cc).
- On "Bootloader Ver": live polling stops (main+0x3618=0). On every "Bootloader Ver" reply the app sends **0x87** (request `87 00 …`, expected reply 0x87, nothing waits) and increments a counter (main+0x361c). On the 5th consecutive reply the counter resets to 0, the version is written to Main.ini and the online firmware check starts. A "Firmware Ver" reply also resets the counter.
  In effect, a device stuck in the bootloader is told to run its app 5 times, then the app offers a firmware download. [H code / M intent]

---------------------------------------------------------------------------------------------------
## 4. 0x82 erase (CMD_ERASE_DEVICE), reply at M+0x61b0 (firmware dialog+0x108)

### Request [H]
`payload = 82 00 00 … 00`. Func 0x424170 writes it inline with WriteFile (same 65-byte format, Sleep 5 ms).

### Sequence [H]
1. `WaitForSingleObject(ev, 5000)` (drain), then `ResetEvent`.
2. Loop: expected = 0x82, send, `WaitForSingleObject(ev, 10000)`. On timeout, resend up to **5 more times** (6 attempts in total). After that, show "CMD_ERASE_DEVICE Timeout" and abort.
3. On a reply, the dispatcher copies 64 bytes and sets expected=0. Then:

### Reply layout [H for byte 2, other bytes unknown]
- `payload[2] == 0x00` → erase OK → go to programming (§5.4)
- `payload[2] != 0` → "Error2!" → abort.

---------------------------------------------------------------------------------------------------
## 5. Firmware update flow

### 5.1 Entry points [H]
- **Manual:** the firmware dialog "Browse" button (0x4237c0) opens a file dialog filtered to "*.hex". The chosen path goes into the edit control (0x3f6), and the app immediately converts HEX→BIN (`0x42c910(bin="<appdir>\Tempbin.bin", hex=<chosen>)`).
  The "Update" button (0x423cd0) requires a non-empty path ("No Update File." otherwise). It disables the buttons and starts worker thread 0x423db0 → 0x423f90.
- **Online:** if the user accepts "An updated firmware version is available. Upgrade now?", the dialog opens with flag +0xb8=1. OnInitDialog (0x423630) then starts thread 0x423dd0. That thread runs `DeleteUrlCacheEntryW(url)` and `URLDownloadToFileW(url = Ver.ini [WH6006] ULR, "<appdir>\Tempbin.hex")`.
  On success it shows "Download Firmware Success.", converts HEX→BIN, then continues as for "Update". On failure it shows the MessageBox "Download Fail".
- OnInitDialog deletes Tempbin.bin and Tempbin.hex. After a successful update, both are deleted again when they came from the online download.

### 5.2 Intel HEX → Tempbin.bin conversion (0x42c910 and helpers) [H]
- Reads the whole .hex file (fopen "rb").
- Validate pass (0x42cb20): every record must start with ':' (else error 1) and contain only hex digits (error 8). The length must match the line length. Record type must be ≤ 5 (error 2). Checksum: `-(sum of bytes after ':' excluding the checksum) == checksum` (error 5). Records are separated by '\n'.
- Extent pass (0x42ce00): `ext` (the upper address) is set by type 02 (`value<<4`), type 04 (`value<<16`), type 03 (16-bit value as-is) and type 05 (32-bit value). Note that types 03/05 **overwrite** the same base variable, which is a bug in the app.
  For type 00, `addr = ext + offset16`. The pass computes `min_addr = min(addr)` and `max_end = max(addr+len)`.
- Buffer = `operator new(max_end - min_addr)`. It is **not pre-filled**: gaps hold heap garbage in the PC app. A reimplementation should fill gaps with 0xFF (erased flash). That choice is [M], not what the app does.
- Fill pass (0x42d050): each data record is copied to `buf[ext + off - min_addr]`.
- **Phantom-byte zeroing:** for every index i with `(i+1) % 4 == 0`, the app sets `buf[i] = 0x00`. So bytes 3, 7, 11, … are forced to 0. This is the PIC24 format: 24-bit instruction plus a zero "phantom" byte per 4-byte group. [H code / M interpretation]
- The buffer is written as raw bytes to Tempbin.bin (fopen "wb"). **File offset 0 = HEX address min_addr.** The rest of the flow assumes offset == HEX byte address, i.e. that min_addr == 0 (the HEX includes the reset vector at 0). [H code / M assumption]
- HEX byte address = 2 × PIC24 program-counter address. [L/M, PIC24 convention, consistent with the >>1 below]

### 5.3 Enter bootloader and check (0x423f90) [H]
1. Disable UI. `Sleep(1)`. Send **`86 00 …`** with no wait.
2. `Sleep(1500)`. Reset the verify offset (global 0x44e6b0 = 0).
3. Send **`80 00 …`** (expected 0x80) and wait 1000 ms. On timeout, resend up to **5 more times** (6 in total). After that, show "CMD_SOFT_VER Timeout", re-enable the UI and set expected=0.
4. On a reply (stored at firmware dialog+0xC8), call 0x423970(state 0): if the version string contains **"Bootloader"**, show it in the status text and go to erase (§4). Otherwise show "Error1!" and abort.

### 5.4 Program (0x4243a0) [H]
- Open `<appdir>\Tempbin.bin` with CFile (modeRead|typeBinary). On failure, show "Open File Error".
- `image = new[0x2A7F9]`. Read **0x2A7F8** bytes into it. A shorter file leaves the tail uninitialised in the PC app. A reimplementation should pad with 0xFF (or 0x00 in phantom bytes) [M].
- `verify_buf = new[0x277F8]`. `LEN = 0x277F8` (161,784 bytes; firmware dialog+0x154). The progress range is 0..2*LEN.
- Expected cmd = 0 (no replies are processed).
- Loop i = 0 .. 0xB49 (**2890 iterations**). `off` starts at **0x3000** and `sent` starts at 0:
  ```
  remaining = LEN - sent
  n = remaining >= 0x38 ? 0x38 : remaining      # 56-byte chunks; remaining<0 → stop
  pc = off >> 1                                  # PIC24 word address
  payload[0]   = 0x83
  payload[1..4]= pc as little-endian u32         # bytes: pc&0xff, pc>>8, pc>>16, pc>>24
  payload[5]   = n
  payload[6..7]= 00 00
  payload[8..8+n) = image[off .. off+n)
  (rest of 64 bytes: leftover from previous chunk; the buffer is zeroed only once, before the loop)
  send; off += n; sent += n
  ```
  LEN / 56 = 2889 exactly, so iteration 2890 sends a **zero-length** 0x83 at pc = 0x2A7F8>>1 = 0x153FC. A reimplementation can probably omit it [M].
  **No ACK, no flow control.** Pacing is only the 5 ms Sleep after each WriteFile.
- Address range written: byte offsets **0x3000 .. 0x2A7F8** (exclusive) of the bin, i.e. **PC 0x1800 .. 0x153FC**. Bin bytes 0..0x2FFF (PC 0..0x17FF) are the bootloader region and are never sent. [H]
  The top of flash beyond 0x2A7F8 (where PIC24FJ128-class config words sit) is never written. [L for the chip identity]
- Then send **`84 00 …`** (no reply), close the file and **Sleep(8000)**.

### 5.5 Verify (0x424670) [H]
- Globals: total = LEN (0x44e6b8, used as the progress base), mismatch flag 0x44e6b4 = 0, verify write index 0x44e6b0 (zeroed in 5.3).
- Chunks: `count = LEN/56 + 1` = 2890. `off` starts at 0x3000. A timeout counter `to` starts at 0.
  ```
  for k in 0..count:
      remaining = LEN - 56*k ; if remaining < 0: break
      n = min(remaining, 0x38)
      payload = 85 | pc LE u32 (pc = off>>1) | n | 00 00 | zeros…   (64 bytes)
      off += n
      if Wait(ev,1000)==TIMEOUT: to++; Sleep(100)
      ResetEvent; expected=0x85; send
      if Wait(ev,1000)==TIMEOUT: to++; Sleep(100)
      ResetEvent; SetEvent
      if to >= 2: mismatch=1; break          # 2 timeouts in total abort the verify
      if mismatch: break
  ```
  The last iteration (k=2889) again requests n=0.

### 5.6 0x85 reply layout (dispatcher 0x42969f) [H]
- `payload[5]` = **n**, the number of data bytes
- `payload[8 .. 8+n)` = flash data (4 bytes per instruction; the phantom byte must read back as 00 to match)
- payload[1..4] and [6..7] are not checked. Assume they echo the request (address) [M].
- Dispatcher: `memcpy(verify_buf + idx, &payload[8], n)`. It compares with `image[0x3000 + idx ..]` (this is the `[edi+0x61f4]+eax+0x3000` compare). On mismatch it sets flag 0x44e6b4=1. Then `idx += n`, the progress bar moves to LEN+…, and it calls SetEvent. Expected stays 0x85.
  **The device must return chunks in request order.** The app tracks the position by a running count, not by the reply address.

### 5.7 Final check and run (0x423970 state 2) [H]
- `memcmp(verify_buf, image+0x3000, LEN)`:
  - equal → `Sleep(1000)`, then send **`87 00 …`** with expected=0 and no wait. Status shows "OK". Temp files are deleted if they were downloaded. The UI is re-enabled and the main window gets WM 0x7ef.
  - different → "Verification failed! Please update the firmware again." On failure the whole sequence (from 5.3) is retried **once** automatically (firmware dialog+0x148 retry counter < 1).

### 5.8 State machine summary
```
[APP running] --80 poll--> "Firmware Ver:x.y.z"
   | user: Update
   v
send 86 ; sleep 1.5s ; (device may re-enumerate as 04D8:003C)
send 80 (1s ×6) --timeout--> "CMD_SOFT_VER Timeout" (abort)
   | reply contains "Bootloader"?  no --> "Error1!" (abort)
   v
send 82 (10s ×6) --timeout--> "CMD_ERASE_DEVICE Timeout" (abort)
   | reply[2]==0 ?  no --> "Error2!" (abort)
   v
2890 × send 83 (56B chunks, PC 0x1800..0x153FC, no ACK, 5 ms apart)
send 84 ; sleep 8 s
2890 × 85 read-back (1 s timeout, abort after 2 timeouts in total)
compare --mismatch--> "Verification failed…" (one automatic retry from 'send 86')
   v
sleep 1 s ; send 87  --> device runs the application
```
Bootloader-at-connect recovery: if the poll sees "Bootloader Ver", the app sends 87 on each poll, and on the 5th it also runs the online firmware check (§6).

---------------------------------------------------------------------------------------------------
## 6. Online update check (threads 0x408090 software, 0x408180 firmware) [H]

Both checks run `DeleteUrlCacheEntryW(url)`, then `URLDownloadToFileW(NULL, url, <file>, 0, NULL)`. Each downloaded file is an **INI file** read with GetPrivateProfileStringW. The file is deleted at startup and after use.

**PC software:** URL `http://download.ecowitt.net/down/softwave?v=SoftwaveReadme.txt` → `<appdir>\Soft.ini`.
- Section `[WeatherLogger]`, keys `VER`, `ULR` (sic), `NOTES`.
- Compare: `_wtoi(VER without '.')` vs `_wtoi("1.7.3" without '.') = 173`. If the server value is greater, the app posts WM 0x7ee → "An updated PC Software version is available. Upgrade now?" + NOTES (';' replaced by '\n').
  Yes → `ShellExecuteW("open", ULR)`. The firmware check is skipped in that case.
- The software check always runs at startup. When it finishes it sets main+0x78e9=1.

**Firmware:** URL `http://download.ecowitt.net/down/filewave?v=FirwaveReadme.txt` → `<appdir>\Ver.ini`.
- This thread waits for the software check to finish. It runs only if the user did not accept the software update (main+0x78e8 && main+0x78e9). It is started from the 0x80 handler (once per connect on "Firmware Ver", or after 5 "Bootloader Ver" replies).
- Section `[WH6006]`, keys `VER`, `ULR` (sic; the firmware .hex URL), `SIZE ` (**with a trailing space** in the key name; stored, but no use was found), `NOTES` (';' → '\n').
- Compare: `_wtoi(device_version without '.')` vs `_wtoi(VER without '.')`. If device < server, the app posts WM 0x7ed → "An updated firmware version is available. Upgrade now?" + NOTES.
  Yes → the firmware dialog opens in online mode and downloads ULR to Tempbin.hex (§5.1).
- Note: dot-stripping makes "1.10.0" (1100) compare greater than "1.9.9" (199). Copy this or improve it as needed.

---------------------------------------------------------------------------------------------------
## 7. PC-side files / Main.ini [H]

All files live in the app directory:
- `Main.ini`:
  - `[Language] Language=English|Deutsch` (read at startup; selects the UI language; written by the language menu)
  - `[About] Web=<url>` and `[Copyright] Copyright=<text>` (read for the About box)
  - `[FirmwareVer] Ver=<x.y.z>` (written with the last device version seen)
- `Ver.ini`, `Soft.ini`: the downloaded update descriptors (§6)
- `Tempbin.hex`, `Tempbin.bin`: temporary firmware files (§5)
- `Log.txt` and `<devicename>Log.txt`: log files (debug text from 0x89 / 0x19 device name). These are outside this area.

---------------------------------------------------------------------------------------------------
## 8. Open questions / not determinable statically
- The 0x80 reply's payload[1] (probably the total length, as in other replies) is unused by the app. [unknown]
- Bytes other than payload[2] in the 0x82 reply, and bytes other than [5] and [8..] in the 0x85 reply. [unknown]
- Whether 0x86 causes a real USB re-enumeration, or the same firmware just switches mode. [unknown; either way the device must be 04D8:003C]
- The flash fill value for gaps: the PC app uses uninitialised heap. 0xFF is recommended.
- WM 0x7ef handler after a successful update (UI only).
