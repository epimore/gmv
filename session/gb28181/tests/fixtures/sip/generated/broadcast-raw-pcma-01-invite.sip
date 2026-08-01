INVITE sip:34020000002000000001@192.0.2.10:25600 SIP/2.0
Via: SIP/2.0/UDP 198.51.100.20:5060;rport;branch=z9hG4bK-broadcast-raw-pcma
Max-Forwards: 70
From: <sip:34020000001110000009@3402000000>;tag=device-broadcast-raw-pcma
To: <sip:34020000002000000001@3402000000>
Contact: <sip:34020000001110000009@198.51.100.20:5060>
Call-ID: broadcast-raw-pcma@gmv.test
CSeq: 230 INVITE
Subject: 34020000001320000102:8203,34020000002000000001:0
Content-Type: application/sdp
Content-Length: 169

v=0
o=34020000001110000009 0 0 IN IP4 198.51.100.20
s=Play
c=IN IP4 198.51.100.20
t=0 0
m=audio 30004 RTP/AVP 8
a=recvonly
a=rtpmap:8 PCMA/8000
f=v/////a/1/8/1
