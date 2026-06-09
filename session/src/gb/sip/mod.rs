mod adapter;
mod dialog;
mod invite;
mod message;
mod register;
mod sdp;
/*
io.rs 收到 bytes
  ↓
sip::adapter::parse()
  ↓
sip::register / message / invite
  ↓
生成响应 bytes
  ↓
io.rs 发送
*/