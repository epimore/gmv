# 这是一个基于GB28181的视频监控实现：兼容2016、2022版本。采用纯RUST语言编码，高效、安全、无惧并发；设备与用户端到端打通、闭环信令服务、流媒体服务。


## 🌟 TCP/UDP端口复用、单机/集群部署、SWAGGER接口文档、不做破坏性更新【接口稳定】、开箱即用【无需编译链接各种依赖】

### 🔗 1. 前端场景界面 demo（VUE 项目）：[simple-app](https://github.com/epimore/simple-app)
### 🔗 2. 自定义业务场景 demo（JAVA 项目）：[simple-biz](https://github.com/epimore/simple-biz)

### ✨✨✨ 在线测试地址：[epimore.cn](https://epimore.cn)

## GMV:SESSION 信令服务已实现：
1. 设备注册、注销、心跳、状态（在线/离线）
2. 设备（子设备）信息、点播/历史回播/视频下载
3. 自动管理流：流注册超时、无人观看、响应超时等自动关闭流
4. 根据cron表达式配置自动采集抓拍实时图像
5. 解析设备告警及推送
6. ...

## GMV:STREAM 流媒体服务已实现
```text
RTP -> PS -> H264 -> HTTP-FLV（直/点播）、MP4（录像）
    -> H264 -> HTTP-FLV（直/点播）、MP4（录像）
    ...
```

### v1版本：完成。
1. 实时播放 - 完成
2. 历史回放 - 完成
   1. 倍数播放 - 完成
   2. 拖动播放 - 完成
3. 云台控制 - 完成
   1. 转向 - 完成
   2. 焦距调整 - 完成
4. 告警推送 - 完成
5. 定时抓拍 - 完成
6. 图片上传 - 完成
7. 视频离线下载 - 完成

## TODO:
### v2版本：预计25年年中启动
// 流媒体服务使用ffmpeg实现,以扩展支持协议减少轮子。
1. 级联 -- SESSION
2. 支持H265,HLS...  -- STREAM
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
![6](./sources/playback.png "历史回放")
![7](./sources/down.png "云端下载")

## 微信交流添加：epimore;备注GMV