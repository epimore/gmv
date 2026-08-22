MESSAGE sip:34020000001110000009@198.51.100.20:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:25600;rport;branch=z9hG4bK-ptz-normal
Max-Forwards: 70
From: <sip:34020000002000000001@3402000000>;tag=platform-ptz-normal
To: <sip:34020000001110000009@198.51.100.20:5060>
Contact: <sip:34020000002000000001@192.0.2.10:25600>
Call-ID: ptz-normal@gmv.test
CSeq: 14 MESSAGE
Content-Type: Application/MANSCDP+xml
Content-Length: 188

<?xml version="1.0" encoding="UTF-8"?>
<Control>
<CmdType>DeviceControl</CmdType>
<SN>14</SN>
<DeviceID>34020000001320000102</DeviceID>
<PTZCmd>A50F0102201000E7</PTZCmd>
</Control>
