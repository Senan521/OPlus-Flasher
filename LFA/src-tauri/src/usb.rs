//! Bridge to the Kotlin UsbPlugin (UsbManager permission + file descriptor).
use serde::{Deserialize, Serialize};
use tauri::{plugin::{Builder, TauriPlugin}, Manager, Runtime};
#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UsbDev { pub name: String, pub vid: u16, pub pid: u16, pub product: String, pub mode: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened { pub fd: i32, pub iface: u8, pub ep_in: u8, pub ep_out: u8, #[serde(default = "d512")] pub mps: u32 }
fn d512() -> u32 { 512 }
#[derive(Deserialize)]
#[allow(dead_code)]
struct PathResp { path: String }
#[allow(dead_code)]
#[derive(Deserialize)]
struct GrantResp { granted: bool }
#[derive(Deserialize)]
struct ListResp { devices: Vec<UsbDev> }

pub struct Usb<R: Runtime> {
    #[cfg(target_os = "android")] h: PluginHandle<R>,
    #[cfg(not(target_os = "android"))] _p: std::marker::PhantomData<R>,
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("usb").setup(|app, api| {
        let _ = &api;
        #[cfg(target_os = "android")]
        { let h = api.register_android_plugin("dev.linuxflasher.usb", "UsbPlugin")?; app.manage(Usb { h }); }
        #[cfg(not(target_os = "android"))]
        app.manage(Usb::<R> { _p: Default::default() });
        Ok(())
    }).build()
}

impl<R: Runtime> Usb<R> {
    pub fn list(&self) -> Result<Vec<UsbDev>, String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<ListResp>("list", ()).map(|r| r.devices).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { let _ = ListResp { devices: vec![] }; Err("USB host yalnızca Android'de çalışır".into()) }
    }
    pub fn storage_granted(&self) -> Result<bool, String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<GrantResp>("storageGranted", ()).map(|r| r.granted).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { Ok(true) }
    }
    pub fn storage_request(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<serde_json::Value>("storageRequest", ()).map(|_| ()).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { Ok(()) }
    }
    pub fn bg_granted(&self) -> Result<bool, String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<GrantResp>("bgGranted", ()).map(|r| r.granted).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { Ok(true) }
    }
    pub fn bg_request(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<serde_json::Value>("bgRequest", ()).map(|_| ()).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { Ok(()) }
    }
    pub fn keep_alive(&self, on: bool) -> Result<(), String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<serde_json::Value>("keepAlive", serde_json::json!({ "on": on })).map(|_| ()).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { let _ = on; Ok(()) }
    }
    pub fn pick(&self, cmd: &str) -> Result<String, String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<PathResp>(cmd, ()).map(|r| r.path).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { let _ = (cmd, PathResp { path: String::new() }); Err("Seçici yalnızca Android’de çalışır".into()) }
    }
    pub fn open(&self, name: &str, proto: u8) -> Result<Opened, String> {
        #[cfg(target_os = "android")]
        { self.h.run_mobile_plugin::<Opened>("open", serde_json::json!({ "name": name, "proto": proto })).map_err(|e| e.to_string()) }
        #[cfg(not(target_os = "android"))]
        { let _ = (name, proto); Err("USB host yalnızca Android'de çalışır".into()) }
    }
}
