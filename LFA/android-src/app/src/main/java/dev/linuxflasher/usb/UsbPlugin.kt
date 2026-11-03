package dev.linuxflasher.usb

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbManager
import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg class KeepArgs { var on: Boolean = false }
@InvokeArg class OpenArgs { lateinit var name: String; var proto: Int = 3 }

@TauriPlugin
class UsbPlugin(private val activity: Activity) : Plugin(activity) {
    private val mgr get() = activity.getSystemService(Context.USB_SERVICE) as UsbManager
    private var conn: UsbDeviceConnection? = null
    private val action = "dev.linuxflasher.USB_PERMISSION"

    private fun mode(d: UsbDevice): String {
        for (i in 0 until d.interfaceCount) {
            val f = d.getInterface(i)
            if (f.interfaceClass == 255 && f.interfaceSubclass == 0x42)
                return when (f.interfaceProtocol) { 3 -> "fastboot"; 1 -> "adb"; else -> "other" }
        }
        return "other"
    }

    @Command fun list(invoke: Invoke) {
        val arr = JSArray()
        for (d in mgr.deviceList.values)
            arr.put(JSObject().put("name", d.deviceName).put("vid", d.vendorId).put("pid", d.productId)
                .put("product", d.productName ?: "USB aygıtı").put("mode", mode(d)))
        invoke.resolve(JSObject().put("devices", arr))
    }

    @Command fun open(invoke: Invoke) {
        val a = invoke.parseArgs(OpenArgs::class.java)
        val d = mgr.deviceList[a.name] ?: return invoke.reject("Aygıt bulunamadı")
        if (mgr.hasPermission(d)) return finish(invoke, d, a.proto)
        val rx = object : BroadcastReceiver() {
            override fun onReceive(c: Context, i: Intent) {
                activity.unregisterReceiver(this)
                if (i.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) finish(invoke, d, a.proto)
                else invoke.reject("USB izni reddedildi")
            }
        }
        val rf = if (Build.VERSION.SDK_INT >= 33) Context.RECEIVER_NOT_EXPORTED else 0
        activity.registerReceiver(rx, IntentFilter(action), rf)
        val pf = if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
        mgr.requestPermission(d, PendingIntent.getBroadcast(activity, 0, Intent(action).setPackage(activity.packageName), pf))
    }

    @Command fun storageGranted(invoke: Invoke) {
        val ok = if (Build.VERSION.SDK_INT >= 30) android.os.Environment.isExternalStorageManager()
                 else activity.checkSelfPermission(android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == android.content.pm.PackageManager.PERMISSION_GRANTED
        invoke.resolve(JSObject().put("granted", ok))
    }
    @Command fun storageRequest(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= 30) {
            try { activity.startActivity(Intent(android.provider.Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + activity.packageName))) }
            catch (e: Exception) { activity.startActivity(Intent(android.provider.Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)) }
        } else activity.requestPermissions(arrayOf(android.Manifest.permission.READ_EXTERNAL_STORAGE, android.Manifest.permission.WRITE_EXTERNAL_STORAGE), 1)
        invoke.resolve(JSObject())
    }
    @Command fun bgGranted(invoke: Invoke) {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as android.os.PowerManager
        val batt = pm.isIgnoringBatteryOptimizations(activity.packageName)
        val notif = Build.VERSION.SDK_INT < 33 || activity.checkSelfPermission("android.permission.POST_NOTIFICATIONS") == android.content.pm.PackageManager.PERMISSION_GRANTED
        invoke.resolve(JSObject().put("granted", batt && notif))
    }
    @Command fun bgRequest(invoke: Invoke) {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as android.os.PowerManager
        if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission("android.permission.POST_NOTIFICATIONS") != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            activity.requestPermissions(arrayOf("android.permission.POST_NOTIFICATIONS"), 2)
        } else if (!pm.isIgnoringBatteryOptimizations(activity.packageName)) {
            try { activity.startActivity(Intent(android.provider.Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + activity.packageName))) }
            catch (e: Exception) { activity.startActivity(Intent(android.provider.Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
        }
        invoke.resolve(JSObject())
    }
    @Command fun keepAlive(invoke: Invoke) {
        val a = invoke.parseArgs(KeepArgs::class.java)
        val i = Intent(activity, KeepAliveService::class.java)
        try { if (a.on) { if (Build.VERSION.SDK_INT >= 26) activity.startForegroundService(i) else activity.startService(i) } else activity.stopService(i) } catch (e: Exception) { }
        invoke.resolve(JSObject())
    }
    @Command fun nativeDir(invoke: Invoke) { invoke.resolve(JSObject().put("path", activity.applicationInfo.nativeLibraryDir)) }
    @Command fun pickFolder(invoke: Invoke) { startActivityForResult(invoke, Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), "pickResult") }
    @Command fun pickFile(invoke: Invoke) {
        startActivityForResult(invoke, Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"), "pickResult")
    }
    @ActivityCallback
    private fun pickResult(invoke: Invoke, result: ActivityResult) {
        val uri = result.data?.data ?: return invoke.reject("Seçim iptal edildi")
        val p = toPath(uri) ?: return invoke.reject("Bu konumdan dosya yolu alınamadı; Dahili depolamadan seç")
        invoke.resolve(JSObject().put("path", p))
    }
    private fun toPath(uri: Uri): String? {
        val id = if (DocumentsContract.isTreeUri(uri)) DocumentsContract.getTreeDocumentId(uri) else DocumentsContract.getDocumentId(uri)
        if (id.startsWith("raw:")) return id.removePrefix("raw:")
        val parts = id.split(":", limit = 2)
        if (parts.size == 2 && (parts[0] == "primary" || Regex("[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}").matches(parts[0]))) {
            val root = if (parts[0] == "primary") "/storage/emulated/0" else "/storage/" + parts[0]
            return if (parts[1].isEmpty()) root else root + "/" + parts[1]
        }
        // İndirilenler sağlayıcısı ("msf:123" veya yalnızca sayı): gerçek yolu MediaStore üzerinden bul
        val mid = if (id.startsWith("msf:")) id.removePrefix("msf:") else if (id.isNotEmpty() && id.all { it.isDigit() }) id else null
        if (mid != null) mediaPath(mid)?.let { return it }
        try { activity.contentResolver.query(uri, arrayOf("_data"), null, null, null)?.use { if (it.moveToFirst()) return it.getString(0) } } catch (_: Exception) {}
        return null
    }
    private fun mediaPath(id: String): String? = try {
        activity.contentResolver.query(android.provider.MediaStore.Files.getContentUri("external"), arrayOf("_data"), "_id=?", arrayOf(id), null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null }
    } catch (_: Exception) { null }

    // Rust claims the interface itself (nusb); here we only open the device and hand over the fd.
    private fun finish(invoke: Invoke, d: UsbDevice, proto: Int) {
        val c = mgr.openDevice(d) ?: return invoke.reject("openDevice başarısız")
        conn?.close(); conn = c
        for (i in 0 until d.interfaceCount) {
            val f = d.getInterface(i)
            if (f.interfaceClass == 255 && f.interfaceSubclass == 0x42 && f.interfaceProtocol == proto) {
                var epIn = -1; var epOut = -1; var mps = 512
                for (e in 0 until f.endpointCount) {
                    val ep = f.getEndpoint(e)
                    if (ep.type == UsbConstants.USB_ENDPOINT_XFER_BULK) {
                        if (ep.direction == UsbConstants.USB_DIR_IN) { epIn = ep.address; mps = ep.maxPacketSize } else epOut = ep.address
                    }
                }
                return invoke.resolve(JSObject().put("fd", c.fileDescriptor).put("iface", f.id).put("epIn", epIn).put("epOut", epOut).put("mps", mps))
            }
        }
        invoke.reject("Uygun USB arayüzü yok (fastboot: bootloader/fastbootd modu, ADB: USB hata ayıklama açık olmalı)")
    }
}
