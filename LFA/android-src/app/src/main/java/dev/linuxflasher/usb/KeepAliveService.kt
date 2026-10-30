package dev.linuxflasher.usb

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

/** Keeps the process alive (foreground service + partial wake lock) while a flash/backup/payload job runs. */
class KeepAliveService : Service() {
    private var wl: PowerManager.WakeLock? = null
    override fun onBind(i: Intent?): IBinder? = null
    override fun onStartCommand(i: Intent?, flags: Int, id: Int): Int {
        val nm = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= 26) nm.createNotificationChannel(NotificationChannel("keep", "OPlus Flasher", NotificationManager.IMPORTANCE_LOW))
        val b = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, "keep") else Notification.Builder(this)
        val n = b.setContentTitle("OPlus Flasher").setContentText("İşlem sürüyor · Operation in progress")
            .setSmallIcon(android.R.drawable.stat_sys_download).setOngoing(true).build()
        if (Build.VERSION.SDK_INT >= 29) startForeground(77, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC) else startForeground(77, n)
        if (wl == null) {
            wl = (getSystemService(POWER_SERVICE) as PowerManager).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "oplus:keep")
            wl?.acquire(3 * 60 * 60 * 1000L)
        }
        return START_NOT_STICKY
    }
    override fun onDestroy() { wl?.let { if (it.isHeld) it.release() }; super.onDestroy() }
}
