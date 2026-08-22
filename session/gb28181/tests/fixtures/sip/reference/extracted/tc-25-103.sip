MESSAGE sip:34020000002000000001@3402000000 SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000141
From: <sip:34020000001320000002@3402000000>;tag=dev0000008c
To: <sip:34020000002000000001@3402000000>
Call-ID: msg-9102-resp@192.168.10.110
CSeq: 1 MESSAGE
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 276

<?xml version="1.0" encoding="GB2312"?>
<Response>
<CmdType>CruiseTrackListQuery</CmdType>
<SN>9102</SN>
<DeviceID>34020000001320000002</DeviceID>
<CruiseTrackList Num="1">
<Item>
<Number>1</Number>
<Name>DefaultTrack</Name>
</Item>
</CruiseTrackList>
</Response>
