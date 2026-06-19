# 优化：
## 1.网络缓冲区
```bash
sudo sysctl -w net.core.rmem_max=16777216
sudo sysctl -w net.core.rmem_default=4194304
### 单机评估
输入：GB28181/RTP/PS
输出：HTTP-FLV
码率：1024 kbps / 路
处理：PS -> FLV 只转封装，不转码
```
| CPU |  内存 | 接入路数 | 观看倍率 | 并发观看路数 |     输入带宽 |     输出带宽 |       总带宽 | 网卡             |
| --: | --: | ---: | ---: | -----: | -------: | -------: | --------: | -------------- |
|  4C | 8GB |  100 |   1x |    100 | 120 Mbps | 120 Mbps |  240 Mbps | 1GbE           |
|  4C | 8GB |  150 |   1x |    150 | 180 Mbps | 180 Mbps |  360 Mbps | 1GbE           |
|  4C | 8GB |  200 |   1x |    200 | 240 Mbps | 240 Mbps |  480 Mbps | 1GbE           |
|  4C | 8GB |  150 |   2x |    300 | 180 Mbps | 360 Mbps |  540 Mbps | 1GbE           |
|  4C | 8GB |  200 |   2x |    400 | 240 Mbps | 480 Mbps |  720 Mbps | 1GbE，接近上限      |
|  4C | 8GB |  200 |   5x |   1000 | 240 Mbps | 1.2 Gbps | 1.44 Gbps | 2.5GbE / 10GbE |
|  8C | 16GB |  300 |   1x |    300 | 360 Mbps | 360 Mbps |  720 Mbps | 1GbE 勉强，建议 2.5GbE    |
|  8C | 16GB |  500 |   1x |    500 | 600 Mbps | 600 Mbps |  1.2 Gbps | 2.5GbE / 10GbE       |
|  8C | 16GB |  500 |   2x |   1000 | 600 Mbps | 1.2 Gbps |  1.8 Gbps | 2.5GbE 接近上限，建议 10GbE |
|  8C | 16GB |  500 |   5x |   2500 | 600 Mbps | 3.0 Gbps |  3.6 Gbps | 10GbE                |
|  8C | 16GB |  800 |   1x |    800 | 960 Mbps | 960 Mbps | 1.92 Gbps | 10GbE                |
...
````
1.异常信息反馈
2.时间调度 |ok
3.net-channel 优化 |ok
4.sip lib select |remaining

cargo fix --allow-dirty --allow-staged
cargo build --release --target x86_64-unknown-linux-gnu
./build-ffmpeg-images.sh
 cross build --target x86_64-unknown-linux-gnu --release
  cross clean --target x86_64-unknown-linux-gnu
./compress.sh
cat logs/gb_2025-11-25.log |grep 'REGISTER sip:34020000001117000219' | sed 's/ data=/\n/; s/\\r/\r/g; s/\\n/\n/g'
run --package gmv-session --bin gmv-session -- start -c ./session/config.yml
1.文档、视频
2.db默认明文，支持加密
3.机器学习
流播放地址：
2. 支持H265,FMP4...  -- STREAM sed
3. 统一响应码
rsmpeg:
src/avcodec/parser.rs
unsafe { ffi::av_parser_init(codec_id as ::std::os::raw::c_int) }
ffmpeg -re -i E:\book\mv\st\yddz.mp4 -vcodec copy -f rtp rtp://172.18.38.186:18568
ffmpeg -re -i E:\code\rust\study\media\rsmpeg\tests\assets\vids\big_buck_bunny_1080p_24fps_h264.h264 -vcodec copy -f rtp rtp://172.18.38.186:18568
ffmpeg -protocol_whitelist file,udp,rtp -i ./123.sdp -c:v copy -f flv 1.flv
ffplay -headers "gmv-token: 1243aaa" "http://172.18.38.186:1857"
cd /mnt/e/code/rust/study/media/rsmpeg/tests/assets/vids/
wget --header="gmv-token: 1243aaa" "http://172.18.38.186:18570/stream-node-1/play/4FEqqz4eqqq0Vzqqq2lsqc4S3Kqqs.flv"
34020000002000000001s2000012345r34020000001111141016

76gujiu04do00

ffplay -headers "gmv-token: user001-gmv-token"-headers "gmv-token: user001-gmv-token" http://172.18.38.186:18570/s1/4FEqqz1Dqsq0Vzqq3K2m0tqq4Zqq6m0s.flv
http://172.18.38.186:18568/s1/4FEqqz1Dqsq0Vzqq3K2m0tqq4Zqq6m0s.flv
````
