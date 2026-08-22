SUBSCRIBE sip:34020000001320000001@192.168.10.110:5060;transport=udp SIP/2.0
Via: SIP/2.0/UDP 192.168.10.10:5060;rport;branch=z9hG4bK-gmv-00000096
From: <sip:34020000002000000001@3402000000>;tag=plt0000005b
To: <sip:34020000001320000001@3402000000>;tag=dev0000005d
Call-ID: sub-catalog-7001@192.168.10.10
CSeq: 2 SUBSCRIBE
Contact: <sip:34020000002000000001@192.168.10.10:5060;transport=udp>
Event: Catalog;id=7001
Expires: 0
Max-Forwards: 70
User-Agent: GMV-GB28181-Platform/2.1.0
Content-Type: Application/MANSCDP+xml
Content-Length: 146

<?xml version="1.0" encoding="GB2312"?>
<Query>
<CmdType>Catalog</CmdType>
<SN>7001</SN>
<DeviceID>34020000001320000001</DeviceID>
</Query>
