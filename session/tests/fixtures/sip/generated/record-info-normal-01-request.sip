MESSAGE sip:34020000001110000009@198.51.100.20:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:25600;rport;branch=z9hG4bK-record-info-normal
Max-Forwards: 70
From: <sip:34020000002000000001@3402000000>;tag=platform-record-info-normal
To: <sip:34020000001110000009@198.51.100.20:5060>
Contact: <sip:34020000002000000001@192.0.2.10:25600>
Call-ID: record-info-normal@gmv.test
CSeq: 12 MESSAGE
Content-Type: Application/MANSCDP+xml
Content-Length: 230

<?xml version="1.0" encoding="UTF-8"?>
<Query>
<CmdType>RecordInfo</CmdType>
<SN>12</SN>
<DeviceID>34020000001110000009</DeviceID>
<StartTime>2026-06-13T00:00:00</StartTime>
<EndTime>2026-06-13T01:00:00</EndTime>
</Query>
