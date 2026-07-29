MESSAGE sip:34020000001110000009@198.51.100.20:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:25600;rport;branch=z9hG4bK-snapshot-business-target
Max-Forwards: 70
From: <sip:34020000002000000001@3402000000>;tag=platform-snapshot-business-target
To: <sip:34020000001320000102@198.51.100.20:5060>
Contact: <sip:34020000002000000001@192.0.2.10:25600>
Call-ID: snapshot-business-target@gmv.test
CSeq: 16 MESSAGE
Content-Type: Application/MANSCDP+xml
Content-Length: 365

<?xml version="1.0" encoding="UTF-8"?>
<Control>
<CmdType>DeviceConfig</CmdType>
<SN>16</SN>
<DeviceID>34020000001320000102</DeviceID>
<SnapShotConfig>
<SnapNum>1</SnapNum>
<Interval>1</Interval>
<UploadURL>http://192.0.2.10:8080/edge/upload/picture/token</UploadURL>
<SessionID>snapshot-business-target-session</SessionID>
</SnapShotConfig>
</Control>
