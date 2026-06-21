MESSAGE sip:34020000002000000001@3402000000 SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000147
From: <sip:34020000001320000002@3402000000>;tag=dev00000092
To: <sip:34020000002000000001@3402000000>
Call-ID: msg-9103-resp@192.168.10.110
CSeq: 1 MESSAGE
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 380

<?xml version="1.0" encoding="GB2312"?>
<Response>
<CmdType>CruiseTrackQuery</CmdType>
<SN>9103</SN>
<DeviceID>34020000001320000002</DeviceID>
<Number>1</Number>
<CruiseTrack>
<Item>
<PresetID>1</PresetID>
<StayTime>10</StayTime>
<Speed>5</Speed>
</Item>
<Item>
<PresetID>2</PresetID>
<StayTime>10</StayTime>
<Speed>5</Speed>
</Item>
</CruiseTrack>
</Response>
