NOTIFY sip:34020000002000000001@192.168.10.10:5060;transport=udp SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000094
From: <sip:34020000001320000001@3402000000>;tag=dev0000005d
To: <sip:34020000002000000001@3402000000>;tag=plt0000005b
Call-ID: sub-catalog-7001@192.168.10.10
CSeq: 1 NOTIFY
Contact: <sip:34020000001320000001@192.168.10.110:5060;transport=udp>
Event: Catalog;id=7001
Subscription-State: active;expires=590
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 312

<?xml version="1.0" encoding="GB2312"?>
<Notify>
<CmdType>Catalog</CmdType>
<SN>7002</SN>
<DeviceID>34020000001320000001</DeviceID>
<SumNum>1</SumNum>
<DeviceList Num="1">
<Item>
<DeviceID>34020000001320000002</DeviceID>
<Event>ON</Event>
<Name>IPC-Channel-1</Name>
</Item>
</DeviceList>
</Notify>
