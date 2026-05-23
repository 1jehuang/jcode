//! 日志模块
//! 
//! 提供信息日志功能

pub mod info {
    //! 信息日志子模块
    
    pub fn log(msg: &str) {
        println!("[INFO] {}", msg);
    }
}
