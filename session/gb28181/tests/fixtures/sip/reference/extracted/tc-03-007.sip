MESSAGE sip:34020000002000000001@3402000000 SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000010
From: <sip:34020000001320000001@3402000000>;tag=dev00000009
To: <sip:34020000002000000001@3402000000>
Call-ID: msg-2001@192.168.10.110
CSeq: 2001 MESSAGE
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 252

<?xml version="1.0" encoding="GB2312"?>
<Notify>
<CmdType>Keepalive</CmdType>
<SN>2001</SN>
<DeviceID>34020000001320000001</DeviceID>
<Status>OK</Status>
<Info>
<DeviceID>34020000001320000002</DeviceID>
<Status>OK</Status>
</Info>
</Notify>
