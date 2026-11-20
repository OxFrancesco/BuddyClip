package dev.buddyclip.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import android.widget.RemoteViews
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

class BuddyClipSyncService : Service() {
    private val mainHandler = Handler(Looper.getMainLooper())
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    override fun onCreate() {
        super.onCreate()
        createChannels()
        registerNetworkWatcher()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopEngineAndSelf()
            return START_NOT_STICKY
        }

        val cfg = SyncRepository.config.value
        if (!cfg.isPaired) {
            stopSelf()
            return START_NOT_STICKY
        }

        val notification = buildStatusNotification(ConnectionPhase.CONNECTING)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        SyncRepository.onServiceStarted()
        startRustEngine(this)
        return START_STICKY
    }

    override fun onTimeout(startId: Int, fgsType: Int) {
        Log.w(TAG, "Foreground service timeout signaled; stopping cleanly")
        stopEngineAndSelf()
    }

    override fun onDestroy() {
        unregisterNetworkWatcher()
        BuddyClipNative.nativeStop()
        SyncRepository.onServiceStopped()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun stopEngineAndSelf() {
        BuddyClipNative.nativeStop()
        SyncRepository.onServiceStopped()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun createChannels() {
        val nm = getSystemService(NotificationManager::class.java) ?: return
        // Clean up legacy low-importance channel so existing installs adopt the silent default-importance channel.
        nm.deleteNotificationChannel(LEGACY_CHANNEL_STATUS)

        val statusChannel = NotificationChannel(
            CHANNEL_STATUS,
            "Connection Status",
            NotificationManager.IMPORTANCE_DEFAULT,
        ).apply {
            description = "Persistent status and one-tap Send Clipboard button"
            setSound(null, null)
            enableVibration(false)
            setShowBadge(false)
        }
        val clipChannel = NotificationChannel(
            CHANNEL_CLIPS,
            "Incoming Clips",
            NotificationManager.IMPORTANCE_DEFAULT,
        ).apply {
            description = "Quiet alert when a clip arrives from your PC"
            setSound(null, null)
            enableVibration(false)
            setShowBadge(false)
        }
        nm.createNotificationChannel(statusChannel)
        nm.createNotificationChannel(clipChannel)
    }

    private fun registerNetworkWatcher() {
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                BuddyClipNative.nativeTriggerReconnect()
            }

            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                    BuddyClipNative.nativeTriggerReconnect()
                }
            }
        }
        val req = NetworkRequest.Builder()
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
        runCatching {
            cm.registerNetworkCallback(req, cb)
            networkCallback = cb
        }.onFailure {
            Log.w(TAG, "registerNetworkCallback failed: ${it.message}")
        }
    }

    private fun unregisterNetworkWatcher() {
        val cb = networkCallback ?: return
        networkCallback = null
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        runCatching { cm.unregisterNetworkCallback(cb) }
    }

    private fun buildStatusNotification(phase: ConnectionPhase): Notification {
        val openIntent = PendingIntent.getActivity(
            this,
            10,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val sendIntent = PendingIntent.getActivity(
            this,
            11,
            Intent(this, SendClipboardActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val statusText = when (phase) {
            ConnectionPhase.CONNECTED -> "Connected"
            ConnectionPhase.CONNECTING -> "Connecting…"
            else -> "Disconnected"
        }

        // Custom RemoteViews keeps the Send Clipboard button directly tappable in the
        // collapsed (unopened) notification view on Android / Samsung One UI.
        val views = RemoteViews(packageName, R.layout.notification_status).apply {
            setTextViewText(R.id.notif_status_text, statusText)
            setOnClickPendingIntent(R.id.notif_send_btn, sendIntent)
        }

        return NotificationCompat.Builder(this, CHANNEL_STATUS)
            .setSmallIcon(R.drawable.ic_tile_clipboard)
            .setColor(BRAND_CORAL)
            .setContentTitle(statusText)
            .setShowWhen(false)
            .setOngoing(true)
            .setSilent(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setContentIntent(openIntent)
            .setStyle(NotificationCompat.DecoratedCustomViewStyle())
            .setCustomContentView(views)
            .setCustomBigContentView(views)
            .build()
    }

    private fun refreshStatusNotification() {
        mainHandler.post {
            val state = SyncRepository.connection.value
            val nm = getSystemService(NotificationManager::class.java) ?: return@post
            nm.notify(NOTIFICATION_ID, buildStatusNotification(state.phase))
        }
    }

    private fun postReceivedClipNotification(item: UiClipItem, copiedDirectly: Boolean) {
        if (!SyncRepository.config.value.showClipNotification) return
        mainHandler.post {
            val copyIntent = PendingIntent.getActivity(
                this,
                21,
                Intent(this, CopyClipActivity::class.java).apply {
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                },
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val title = if (copiedDirectly) {
                "Copied from ${item.originName}"
            } else {
                "Tap to copy from ${item.originName}"
            }
            val n = NotificationCompat.Builder(this, CHANNEL_CLIPS)
                .setSmallIcon(R.drawable.ic_tile_clipboard)
                .setColor(BRAND_CORAL)
                .setContentTitle(title)
                .setContentText(item.preview(64))
                .setShowWhen(false)
                .setAutoCancel(true)
                .setSilent(true)
                .setTimeoutAfter(8_000)
                .setContentIntent(copyIntent)
                .build()
            val nm = getSystemService(NotificationManager::class.java) ?: return@post
            nm.notify(CLIP_NOTIFICATION_ID, n)
        }
    }

    companion object {
        private const val TAG = "buddyclip-svc"
        private const val BRAND_CORAL = 0xFFD6544B.toInt()
        private const val LEGACY_CHANNEL_STATUS = "buddyclip_status"
        private const val CHANNEL_STATUS = "buddyclip_status_v2"
        private const val CHANNEL_CLIPS = "buddyclip_clips"
        private const val NOTIFICATION_ID = 1001
        private const val CLIP_NOTIFICATION_ID = 1002
        private const val ACTION_STOP = "dev.buddyclip.app.STOP"

        @Volatile
        private var activeService: BuddyClipSyncService? = null

        fun startOrRestart(context: Context) {
            val cfg = SyncRepository.config.value
            if (!cfg.isPaired) return
            val intent = Intent(context, BuddyClipSyncService::class.java)
            runCatching {
                ContextCompat.startForegroundService(context, intent)
            }.onFailure { e ->
                Log.w(TAG, "startForegroundService failed: ${e.message}")
            }
        }

        fun stop(context: Context) {
            val intent = Intent(context, BuddyClipSyncService::class.java).apply {
                action = ACTION_STOP
            }
            runCatching { context.stopService(intent) }
            BuddyClipNative.nativeStop()
            SyncRepository.onServiceStopped()
        }

        fun ensureEngineStarted(context: Context) {
            val cfg = SyncRepository.config.value
            if (!cfg.isPaired) return
            startRustEngine(activeService, context.applicationContext)
        }

        private fun startRustEngine(service: BuddyClipSyncService?, fallbackContext: Context? = null) {
            if (service != null) activeService = service
            val ctx = service?.applicationContext ?: fallbackContext ?: return
            val cfg = SyncRepository.config.value
            if (!cfg.isPaired) return

            val callback = object : BuddyClipNativeCallback {
                override fun onStateChanged(state: String, detail: String, fingerprint: String) {
                    SyncRepository.onNativeStateChanged(state, detail, fingerprint)
                    activeService?.refreshStatusNotification()
                }

                override fun onClipReceived(
                    id: String,
                    originName: String,
                    mimeType: String,
                    payload: ByteArray,
                    createdAtMs: Long,
                ) {
                    val item = SyncRepository.onNativeClipReceived(
                        id = id,
                        originName = originName,
                        mimeType = mimeType,
                        payload = payload,
                        createdAtMs = createdAtMs,
                    ) ?: return
                    val copied = if (SyncRepository.config.value.autoWriteClipboard) {
                        var wrote = false
                        val latch = java.util.concurrent.CountDownLatch(1)
                        Handler(Looper.getMainLooper()).post {
                            wrote = SyncRepository.writeClipToSystemClipboard(ctx, item)
                            latch.countDown()
                        }
                        runCatching { latch.await(800, java.util.concurrent.TimeUnit.MILLISECONDS) }
                        wrote
                    } else {
                        false
                    }
                    activeService?.refreshStatusNotification()
                    activeService?.postReceivedClipNotification(item, copied)
                }

                override fun onClipSent(clipId: String) {
                    Log.d(TAG, "Encrypted clip sent: $clipId")
                }

                override fun onHistoryCleared(clearedAtMs: Long) {
                    SyncRepository.onNativeHistoryCleared(clearedAtMs)
                }
            }

            val err = BuddyClipNative.nativeStart(
                host = cfg.host,
                port = cfg.port,
                keyBase64 = cfg.keyBase64,
                deviceId = cfg.deviceId,
                deviceName = cfg.deviceName,
                clearedAtMs = cfg.historyClearedAtMs,
                callback = callback,
            )
            if (err != null) {
                SyncRepository.onNativeStateChanged("error", err, "")
            }
        }
    }
}
