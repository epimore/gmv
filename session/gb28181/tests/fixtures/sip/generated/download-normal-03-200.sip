SIP/2.0 200 OK
Via: SIP/2.0/UDP 192.0.2.10:25600;rport=25600;branch=z9hG4bK-download-normal
From: <sip:34020000002000000001@3402000000>;tag=platform-download-normal
To: <sip:34020000001110000009@198.51.100.20:5060>;tag=device-download-normal
Call-ID: download-normal@gmv.test
CSeq: 22 INVITE
Contact: <sip:34020000001110000009@198.51.100.20:5060>
Content-Type: application/sdp
Content-Length: 167

v=0
o=34020000001110000009 0 0 IN IP4 198.51.100.20
s=Play
c=IN IP4 198.51.100.20
t=0 0
m=video 30000 RTP/AVP 96
a=sendonly
a=rtpmap:96 PS/90000
y=0100008199
