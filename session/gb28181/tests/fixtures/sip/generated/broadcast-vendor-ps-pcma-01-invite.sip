INVITE sip:34020000002000000001@192.0.2.10:25600 SIP/2.0
Via: SIP/2.0/UDP 198.51.100.20:5060;rport;branch=z9hG4bK-broadcast-vendor-ps-pcma
Max-Forwards: 70
From: <sip:34020000001110000009@3402000000>;tag=device-broadcast-vendor-ps-pcma
To: <sip:34020000002000000001@3402000000>
Contact: <sip:34020000001110000009@198.51.100.20:5060>
Call-ID: broadcast-vendor-ps-pcma@gmv.test
CSeq: 231 INVITE
Subject: 34020000001320000102:8204,34020000002000000001:0
Content-Type: application/sdp
Content-Length: 174

v=0
o=34020000001110000009 0 0 IN IP4 192.168.110.254
s=Play
c=IN IP4 192.168.110.254
t=0 0
m=audio 63086 RTP/AVP 96
a=recvonly
a=rtpmap:96 PS/90000
f=v/////a/1/8/1
