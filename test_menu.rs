use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

fn main() {
    let tray_menu = Menu::new();
    
    let model_menu = Submenu::new("选择模型", true);
    
    let item1 = CheckMenuItem::with_id("m1", "模型1", true, true, None);
    let item2 = CheckMenuItem::with_id("m2", "模型2", true, false, None);
    
    let _ = model_menu.append(&item1);
    let _ = model_menu.append(&item2);
    
    let _ = tray_menu.append(&model_menu);
    println!("Menu created.");
}
