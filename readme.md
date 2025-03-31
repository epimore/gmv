# 这是一个基于GB28181的视频监控实现：兼容2016、2022版本。采用纯RUST语言编码，高效、安全、无惧并发。

## 🌟 TCP/UDP端口复用、单机/集群部署、SWAGGER接口文档、不做破坏性更新【接口稳定】、开箱即用【无需编译链接各种依赖】

### 🔗 1. 前端场景界面 demo（VUE 项目）：[simple-app](https://github.com/epimore/simple-app)
### 🔗 2. 自定义业务场景 demo（JAVA 项目）：[simple-biz](https://github.com/epimore/simple-biz)


## GMV:SESSION 信令服务实现：
1. 设备注册
2. 设备心跳
3. 状态信息（在线/离线）
4. 设备信息查询
5. 设备代理通道目录信息查询
6. 实现点播；
7. 自动关闭流：流注册超时、无人观看、响应超时等
8. 支持与GMV:STREAM多节点部署通信

## GMV:STREAM 流媒体服务实现：
1. RTP流解封装
2. PS流解封装
3. 提取H264视频帧
4. 封装H264视频帧到FLV
5. 按需监听SSRC及实现如上（1-5）
6. 实现HTTP-FLV

## TODO:
### v1版本：预计25年一季度完成。
1. 历史回放 - 完成
    1. 倍数播放 - 完成
    2. 拖动播放 - 完成
2. 云台控制 - 完成
    1. 转向 - 完成
    2. 焦距调整 - 完成
3. 事件配置 - 完成
4. 定时抓拍 - 完成
5. 图片上传 - 完成
6. 视频下载
### v2版本：预计25年年中启动
1. 级联
2. 支持H265,HLS
3. 统一响应码
### V3版本：预计25年底启动
1. 按需推流
2. 图片AI识别-插件化业务场景
3. 多数据库配置

![0](./sources/swagger.png "API文档")
![1](./sources/d_list.png "设备目录")
![2](./sources/d_add.png "设备添加")
![3](./sources/c_list.png "设备目录通道")
![4](./sources/c_d_list.png "通道目录操作")
![5](./sources/c_play.png "通道点播")
![5](./sources/playback.png "历史回放")