MESSAGE sip:34020000002000000001@3402000000 SIP/2.0
Via: SIP/2.0/UDP 192.168.10.110:5060;rport;branch=z9hG4bK-gmv-00000129
From: <sip:34020000001320000002@3402000000>;tag=dev00000080
To: <sip:34020000002000000001@3402000000>
Call-ID: msg-9003-resp@192.168.10.110
CSeq: 1 MESSAGE
Max-Forwards: 70
User-Agent: IPC-GB28181-UA/2022
Content-Type: Application/MANSCDP+xml
Content-Length: 396

<?xml version="1.0" encoding="GB2312"?>
<Response>
<CmdType>ConfigDownload</CmdType>
<SN>9003</SN>
<DeviceID>34020000001320000002</DeviceID>
<Result>OK</Result>
<SnapShotConfig>
<SnapNum>2</SnapNum>
<Interval>1</Interval>
<UploadURL>http://192.168.10.10:8080/gb28181/snapshot/upload</UploadURL>
<SessionID>snap202606141200000000000000000001</SessionID>
</SnapShotConfig>
</Response>
