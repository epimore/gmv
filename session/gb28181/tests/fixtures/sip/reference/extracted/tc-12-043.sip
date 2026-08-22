MESSAGE sip:34020000002000000001@3402000000 SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000062
From: <sip:34020000001340000001@3402000000>;tag=dev0000003d
To: <sip:34020000002000000001@3402000000>
Call-ID: msg-5001@192.168.10.110
CSeq: 5001 MESSAGE
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 408

<?xml version="1.0" encoding="GB2312"?>
<Notify>
<CmdType>Alarm</CmdType>
<SN>5001</SN>
<DeviceID>34020000001340000001</DeviceID>
<AlarmPriority>1</AlarmPriority>
<AlarmMethod>4</AlarmMethod>
<AlarmTime>2026-06-14T12:01:02</AlarmTime>
<AlarmDescription>VideoLost</AlarmDescription>
<Longitude>117.2800</Longitude>
<Latitude>31.8600</Latitude>
<Info>
<AlarmType>1</AlarmType>
</Info>
</Notify>
