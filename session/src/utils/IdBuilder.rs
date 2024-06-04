use std::time::{SystemTime, UNIX_EPOCH};
use common::err::GlobalResult;
use crate::storage::entity::GmvOauth;

const D_DIC: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
//按键盘从上至下，从左到右形成小写、大写字母字典表
const A_DIC: [char; 52] = ['q', 'a', 'z', 'w', 's', 'x', 'e', 'd', 'c', 'r', 'f', 'v', 't', 'g', 'b', 'y', 'h', 'n', 'u', 'j', 'm', 'i', 'k', 'o', 'l', 'p', 'Q', 'A', 'Z', 'W', 'S', 'X', 'E', 'D', 'C', 'R', 'F', 'V', 'T', 'G', 'B', 'Y', 'H', 'N', 'U', 'J', 'M', 'I', 'K', 'O', 'L', 'P'];

//生成stream_id,参数由调用方校验,简单对称加密算法
// device_id 20位十进制纯数字
// channel_id 20位十进制纯数字
// ssrc 10位十进制纯数字
pub fn en_stream_id(device_id: &str, channel_id: &str, ssrc: &str) -> String {
    let ori_key = format!("{device_id}{channel_id}{ssrc}");
    //转换为二进制字符串: 50*4=200位
    let mut tmp_key0 = String::new();
    for ch in ori_key.chars() {
        let digit = ch.to_digit(10).expect("Invalid digit");
        tmp_key0.push_str(&format!("{:04b}", digit));
    }
    //使用纳秒的后两位生成填充字符串,并取7个字符
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let nanos = since_the_epoch.as_nanos();
    let fill_str = format!("{:07b}", nanos % 100);
    let mut fill = fill_str.chars();
    //插入7个数进行填充,200+7=207,便于按9位整除分组
    let mut tmp_key1 = String::new();
    for (i, ch) in tmp_key0.chars().enumerate() {
        tmp_key1.push_str(&ch.to_string());
        //跳过第一个23后,每隔23位且小于200,满足有且插入7个数
        if i > 23 && i % 23 == 0 {
            tmp_key1.push_str(&fill.next().unwrap().to_string());
        }
    }
    //按每9位为一组进行分组,且每组数字再分为3个子分组,子分组左侧值与右侧值交换位置
    let chunks: Vec<String> = tmp_key1
        .chars()
        .collect::<Vec<_>>()
        .chunks_mut(9)
        .map(|chunk0| {
            chunk0.chunks_mut(3).map(|item| {
                item.swap(0, 2);
            }).count();
            chunk0.iter().collect()
        }).collect();
    //生成最终的key:当商大于0时,取数字字典进行填充
    let mut dst_key = String::new();
    for chunk in chunks {
        let val = usize::from_str_radix(&chunk, 2).expect("Invalid binary group");
        let circle = val / 52;
        let index = val % 52;
        if circle > 0 {
            dst_key.push_str(&D_DIC[circle - 1].to_string());
        }
        dst_key.push_str(&A_DIC[index].to_string());
    }
    dst_key
}

//返回(device_id,channel_id,ssrc)
pub fn de_stream_id(stream_id: &str) -> (String, String, String) {
    let mut tmp_key0 = String::new();
    let mut pre = 0;
    for ch in stream_id.chars() {
        if let Some(circle) = ch.to_digit(10) {
            pre = (circle + 1) * 52;
        } else {
            let a_index = A_DIC.iter().position(|a| a == &ch).expect("非法字符");
            let digit = pre as usize + a_index;
            tmp_key0.push_str(&format!("{:09b}", digit));
            pre = 0;
        }
    }
    let tmp_key1 = tmp_key0
        .chars()
        .collect::<Vec<_>>()
        .chunks_mut(3)
        .map(|item| {
            item.swap(0, 2);
            item.iter()
        }).flatten().collect::<String>();
    let mut ti = 23 + 23 + 1;
    let bin_str = tmp_key1.chars()
        .enumerate()
        .filter_map(|(index, ch)| {
            if index == ti {
                ti += 23 + 1;
                None
            } else {
                Some(ch)
            }
        }).collect::<String>();
    let ori_str = bin_str
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| {
            format!("{}", u32::from_str_radix(chunk.iter().collect::<String>().as_str(), 2).expect("Invalid binary group"))
        }).collect::<String>();
    (ori_str[0..20].to_string(), ori_str[20..40].to_string(), ori_str[40..].to_string())
}

pub fn build_ssrc(device_id:&String,channel_id:&String,num_ssrc:u16,live:bool)->GlobalResult<String>{
    let result = GmvOauth::read_gmv_oauth_by_device_id(device_id)?;
    unimplemented!()
}

#[test]
fn test1() {
    let device_id = "34020000001110000001";
    let channel_id = "34020000001320000101";
    let ssrc = "1100000001";
    let stream_id = en_stream_id(device_id, channel_id, ssrc);
    let stream_id1 = en_stream_id(device_id, channel_id, ssrc);
    let stream_id2 = en_stream_id(device_id, channel_id, ssrc);
    let stream_id3 = en_stream_id(device_id, channel_id, ssrc);
    let stream_id4 = en_stream_id(device_id, channel_id, ssrc);
    println!("stream_id1 = {}", &stream_id1);
    println!("stream_id2 = {}", &stream_id2);
    println!("stream_id3 = {}", &stream_id3);
    println!("stream_id4 = {}", &stream_id4);
    let (d_d_id,d_c_id,d_ssrc) = de_stream_id(&stream_id);
    println!("stream_id = {}", &stream_id);
    assert_eq!(device_id,&d_d_id[..]);
    assert_eq!(channel_id,&d_c_id[..]);
    assert_eq!(ssrc,&d_ssrc[..]);
}