use std::io;

fn main() -> io::Result<()> {
    // 只有在编译目标是 windows 时才执行
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        // 设置刚才生成的 ico 文件
        res.set_icon("icon.ico");
        res.compile()?;
    }
    Ok(())
}
