MESSAGE sip:34020000002000000001@3402000000 SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000068
From: <sip:34020000001320000002@3402000000>;tag=dev00000043
To: <sip:34020000002000000001@3402000000>
Call-ID: msg-6001-resp@192.168.10.110
CSeq: 1 MESSAGE
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 625

<?xml version="1.0" encoding="GB2312"?>
<Response>
<CmdType>RecordInfo</CmdType>
<SN>6001</SN>
<DeviceID>34020000001320000002</DeviceID>
<Name>IPC-Channel-1</Name>
<SumNum>1</SumNum>
<RecordList Num="1">
<Item>
<DeviceID>34020000001320000002</DeviceID>
<Name>record-20260614-100000</Name>
<FilePath>file:///record/20260614/100000.ps</FilePath>
<Address>Gate-01</Address>
<StartTime>2026-06-14T10:00:00</StartTime>
<EndTime>2026-06-14T11:00:00</EndTime>
<Secrecy>0</Secrecy>
<Type>time</Type>
<RecorderID>34020000001320000001</RecorderID>
<FileSize>268435456</FileSize>
</Item>
</RecordList>
</Response>
