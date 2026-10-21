# Logger firmware (WS6006 V1.1.39)

Source: `WS6006_V1.1.39_202505211148.hex` (PIC24, program memory 0x0000-0x2B000). These notes come from the text strings in the image, read through the PSV window: the low 16 bits of each 24-bit word. The code itself has not been disassembled yet, so the meaning of each string is inferred from its text and its neighbours. Confidence: H = the string states it directly, M = inferred from context.

## Device log (0x89)

The firmware forwards its modem's AT traffic and status text to the PC as unsolicited `0x89` packets. The text is at `p[3 .. 3+p[2]]`, and line breaks are embedded in the text, not tied to packet boundaries. The Windows app appends these packets to `C:\Users\Public\WeatherLogger\<device name>Log.txt`. The default device name is the modem IMEI. `wh6006 log` and the GUI's "Device log" tab show the same stream.

A real start-up sequence, captured by the Windows app from a logger with no SIM:

```
+NWTYPEIND:31 / +NWTYPEIND:34 / +SIGNALIND:5
ATI -> Manufacturer: MEIG INCORPORATED, Model: SLM750-VE, Revision: SLM750-V_4.0.4_EQ101
AT+CGSN -> <IMEI>
at+nvrw=1,10,"001F" / AT+BNDLOCK=2,0
AT+CPIN? -> +CME ERROR: SIM not inserted
```

## Modem

The firmware supports several modems: MeiG SLM750 (4G, the one in this unit), Quectel UC20/UC15 and EG9xx (`4G_EG9XX`), and Luat Air795 (`4G_AIR795`). It picks the AT dialect from the `ATI` revision string. The APN is applied with whichever of `AT+CGDCONT`, `AT+QICSGP`, `AT+CPNETAPN` or `AT$QCPDPP` the modem understands. (H)

There is no SIM PIN handling. The firmware only checks `AT+CPIN?` for `READY`, `NOT READY` and `NOT INSERTED`. A PIN-locked SIM will not work. (M)

Network time comes from `pool.ntp.org` or `time.windows.com` over UDP. (H)

## Uploads

Each service is a plain HTTP request over a TCP socket opened by the modem. (H)

| Service | Request |
|---|---|
| Wunderground | `GET /weatherstation/updateweatherstation.php?ID=…&PASSWORD=…&indoortempf=…&tempf=…&dateutc=…&softwaretype=…%20V1.1.39&action=updateraw&realtime=1&rtfreq=5 HTTP/1.0` to `rtupdate.wunderground.com` |
| Custom ("User Server", sub 3) | The same Wunderground format, sent to the configured host and port, with the path ending `.php?`, `.jsp?` or `.asp?` set by the server type (0x0c byte 3) |
| Weathercloud | `GET /v01/set?wid=…&key=…&tempin=…&temp=…&date=…&time=…&type=…&ver=1.1.39` to `api.weathercloud.net` |
| WOW | `GET /automaticreading?siteid=…&siteAuthenticationKey=…&baromin=…&softwaretype=…%201.1.39` to `wow.metoffice.gov.uk` |
| Ecowitt | `POST /data/report/` to `cdnrtpdate.ecowitt.net`, form-encoded `PASSKEY=…&stationtype=WS6006_V1.1.39&dateutc=…&imei=…&imsi=…&mversion=…&up_mtd=<n>G&model=WS6006_…&freq=…` plus Ecowitt field names (`tempinf`, `baromrelin`, `wh65batt`, …) |

Rules stated by the firmware's own status text (H):

- "The upload function is turned off. Because the upload interval is set to 0." An interval of 0 disables every upload.
- "ID or Password is null." A service with an empty station ID or password is not uploaded. This is the only per-service off switch.
- "The web sever is null." The custom server is skipped when its host is empty.
- The status reply lists "Succeed" or "Fail" for Wunderground, WeatherClould [sic], WOW and User Sever [sic].

## Battery (H)

- Below 10 %: "Warning: Battery power is less than 10%. Device will soon suspend internet uploading and SMS."
- Below 5 %: "Internet uploading and SMS are suspended."
- Back at 60 %: "Battery power restored to 60%. Restart RF and data Upload."

## SMS commands (M)

These words appear next to the SMS reply texts and are probably the commands the logger accepts by SMS: `status`, `current`, `switch`, `alarm`, `interval`, `telephone` and `reboot`. The replies are built from labels such as "Upload status", "Other status", "Current(1/2)", "Max(1/2)", "Switch(1/2)", "Telephone TEL1:", "Alarm enabled" and "Alarm disabled". The exact syntax, and whether the sender's number must be one of TEL1-3, is not confirmed.

## SD card

Daily files are named `20YYMMDD.CSV`. The header row is `Time,Indoor Temperature(<unit>),Indoor Humidity(%),Outdoor Temperature(<unit>),…,Solar Rad(<unit>),UVI,Soilmoisture(%),PM2.5(ug/m3),leafwetness(%),Temperature(<unit>)`, and rows begin with `20YY/MM/DD H:MM:SS`. (H)
