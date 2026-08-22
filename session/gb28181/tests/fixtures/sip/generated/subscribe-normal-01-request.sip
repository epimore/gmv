SUBSCRIBE sip:34020000001110000009@198.51.100.20:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:25600;rport;branch=z9hG4bK-subscribe-normal
Max-Forwards: 70
From: <sip:34020000002000000001@3402000000>;tag=platform-subscribe-normal
To: <sip:34020000001110000009@198.51.100.20:5060>
Contact: <sip:34020000002000000001@192.0.2.10:25600>
Call-ID: subscribe-normal@gmv.test
CSeq: 27 SUBSCRIBE
Event: Catalog
Expires: 300
Content-Type: Application/MANSCDP+xml
Content-Length: 143

<?xml version="1.0" encoding="UTF-8"?>
<Query>
<CmdType>Catalog</CmdType>
<SN>27</SN>
<DeviceID>34020000001110000009</DeviceID>
</Query>
