MESSAGE sip:34020000002000000001@192.0.2.10:25600 SIP/2.0
Via: SIP/2.0/UDP 198.51.100.20:5060;rport;branch=z9hG4bK-keepalive-normal
Max-Forwards: 70
From: <sip:34020000001110000009@3402000000>;tag=device-keepalive-normal
To: <sip:34020000002000000001@3402000000>
Contact: <sip:34020000001110000009@198.51.100.20:5060>
Call-ID: keepalive-normal@gmv.test
CSeq: 3 MESSAGE
Content-Type: Application/MANSCDP+xml
Content-Length: 167

<?xml version="1.0" encoding="UTF-8"?>
<Notify>
<CmdType>Keepalive</CmdType>
<SN>3</SN>
<DeviceID>34020000001110000009</DeviceID>
<Status>OK</Status>
</Notify>
