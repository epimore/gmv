# 单机性能评估
```bash
网络缓冲区优化
sudo sysctl -w net.core.rmem_max=16777216
sudo sysctl -w net.core.rmem_default=4194304
```
```bash
输入：GB28181/RTP/PS
输出：HTTP-FLV
码率：1024 kbps / 路
处理：PS -> FLV 只转封装，不转码
```
| CPU |  内存 | 并发接入路数 | 观看倍率 | 并发观看路数 |     输入带宽 |     输出带宽 |       总带宽 | 网卡             |
| --: | --: | -----: | ---: | -----: | -------: | -------: | --------: | -------------- |
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

