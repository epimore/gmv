MESSAGE sip:34020000001110000009@198.51.100.20:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:25600;rport;branch=z9hG4bK-device-info-normal
Max-Forwards: 70
From: <sip:34020000002000000001@3402000000>;tag=platform-device-info-normal
To: <sip:34020000001110000009@198.51.100.20:5060>
Contact: <sip:34020000002000000001@192.0.2.10:25600>
Call-ID: device-info-normal@gmv.test
CSeq: 10 MESSAGE
Content-Type: Application/MANSCDP+xml
Content-Length: 146

<?xml version="1.0" encoding="UTF-8"?>
<Query>
<CmdType>DeviceInfo</CmdType>
<SN>10</SN>
<DeviceID>34020000001110000009</DeviceID>
</Query>
