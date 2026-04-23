fn main() { 
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, IDC_IBEAM, LoadCursorW}; 
    let mut ci: CURSORINFO = unsafe { std::mem::zeroed() }; 
    ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32; 
    unsafe { GetCursorInfo(&mut ci) }; 
    let ibeam = unsafe { LoadCursorW(0 as _, IDC_IBEAM) }; 
    println!("hCursor: {:?}, ibeam: {:?}", ci.hCursor, ibeam); 
}
