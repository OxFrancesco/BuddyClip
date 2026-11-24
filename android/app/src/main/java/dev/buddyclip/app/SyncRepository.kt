package dev.buddyclip.app

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.Build
import android.util.Log
import androidx.core.content.FileProvider
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File
import java.security.MessageDigest

enum class ConnectionPhase {
    UNPAIRED,
    STOPPED,
    CONNECTING,
    CONNECTED,
    DISCONNECTED,
    ERROR,
}

data class ConnectionUiState(
    val phase: ConnectionPhase = ConnectionPhase.UNPAIRED,
    val detail: String = "Not paired yet",
    val fingerprint: String = "",
    val serviceRunning: Boolean = false,
)

data class UiClipItem(
    val id: String,
    val originName: String,
    val mimeType: String,
    val payload: ByteArray,
    val createdAtMs: Long,
    val isFromLocal: Boolean,
) {
    val isText: Boolean get() = mimeType.startsWith("text/")
    val isImage: Boolean get() = mimeType.startsWith("image/")

    val textValue: String?
        get() = if (isText) String(payload, Charsets.UTF_8) else null

    fun preview(maxChars: Int = 120): String {
        val txt = textValue
        if (txt != null) {
            val cleaned = txt.replace(Regex("[\\r\\n\\t]+"), " ").trim()
            return if (cleaned.length > maxChars) cleaned.take(maxChars) + "…" else cleaned
        }
        if (isImage) {
            val kb = payload.size / 1024.0
            return String.format("[%s · %.1f KB]", mimeType.substringAfter('/'), kb)
        }
        return "[$mimeType · ${payload.size} B]"
    }
}

sealed class SendClipOutcome {
    data class Sent(val preview: String, val targetName: String) : SendClipOutcome()
    data object Unchanged : SendClipOutcome()
    data object Empty : SendClipOutcome()
    data object SensitiveSkipped : SendClipOutcome()
    data class Failed(val reason: String) : SendClipOutcome()
}

object SyncRepository {
    private const val TAG = "buddyclip-repo"
    private const val MAX_HISTORY = 25
    private const val MAX_PAYLOAD_BYTES = 16 * 1024 * 1024

    private lateinit var appContext: Context
    private lateinit var store: SecureConfigStore

    private val _config = MutableStateFlow(BuddyClipConfig())
    val config: StateFlow<BuddyClipConfig> = _config.asStateFlow()

    private val _connection = MutableStateFlow(ConnectionUiState())
    val connection: StateFlow<ConnectionUiState> = _connection.asStateFlow()

    private val _history = MutableStateFlow<List<UiClipItem>>(emptyList())
    val history: StateFlow<List<UiClipItem>> = _history.asStateFlow()

    @Volatile
    private var lastSyncedDigest: String? = null

    fun init(context: Context) {
        appContext = context.applicationContext
        store = SecureConfigStore(appContext)
        val loaded = store.load()
        _config.value = loaded
        if (!loaded.isPaired) {
            _connection.value = ConnectionUiState(
                phase = ConnectionPhase.UNPAIRED,
                detail = "Scan your PC QR code to pair",
            )
        } else {
            val fp = runCatching { BuddyClipNative.nativeKeyFingerprint(loaded.keyBase64) }.getOrDefault("")
            _connection.value = ConnectionUiState(
                phase = ConnectionPhase.STOPPED,
                detail = "Paired with ${loaded.hostName.ifBlank { loaded.host }}",
                fingerprint = fp,
            )
        }
    }

    fun updateConfig(newConfig: BuddyClipConfig, restartServiceIfPaired: Boolean = true) {
        store.save(newConfig)
        _config.value = newConfig
        val fp = if (newConfig.keyBase64.isNotBlank()) {
            runCatching { BuddyClipNative.nativeKeyFingerprint(newConfig.keyBase64) }.getOrDefault("")
        } else {
            ""
        }
        if (!newConfig.isPaired) {
            BuddyClipSyncService.stop(appContext)
            _connection.value = ConnectionUiState(
                phase = ConnectionPhase.UNPAIRED,
                detail = "Scan your PC QR code to pair",
            )
            return
        }
        _connection.value = _connection.value.copy(fingerprint = fp)
        if (restartServiceIfPaired) {
            BuddyClipSyncService.startOrRestart(appContext)
        }
    }

    fun unpair() {
        BuddyClipSyncService.stop(appContext)
        store.clear()
        clearHistory()
        val fresh = BuddyClipConfig()
        _config.value = fresh
        _connection.value = ConnectionUiState(
            phase = ConnectionPhase.UNPAIRED,
            detail = "Unpaired",
        )
    }

    fun onServiceStarted() {
        _connection.value = _connection.value.copy(serviceRunning = true)
    }

    fun onServiceStopped() {
        val paired = _config.value.isPaired
        _connection.value = _connection.value.copy(
            serviceRunning = false,
            phase = if (paired) ConnectionPhase.STOPPED else ConnectionPhase.UNPAIRED,
            detail = if (paired) "Sync paused" else "Not paired yet",
        )
    }

    fun onNativeStateChanged(state: String, detail: String, fingerprint: String) {
        val phase = when (state) {
            "connecting" -> ConnectionPhase.CONNECTING
            "connected" -> ConnectionPhase.CONNECTED
            "disconnected" -> ConnectionPhase.DISCONNECTED
            "error" -> ConnectionPhase.ERROR
            else -> ConnectionPhase.DISCONNECTED
        }
        val effectiveFp = fingerprint.ifBlank { _connection.value.fingerprint }
        _connection.value = _connection.value.copy(
            phase = phase,
            detail = detail,
            fingerprint = effectiveFp,
            serviceRunning = true,
        )
        BuddyClipTileService.requestUpdate(appContext)
    }

    fun onNativeClipReceived(
        id: String,
        originName: String,
        mimeType: String,
        payload: ByteArray,
        createdAtMs: Long,
    ): UiClipItem? {
        if (createdAtMs <= _config.value.historyClearedAtMs) {
            return null
        }
        val item = UiClipItem(
            id = id,
            originName = originName,
            mimeType = mimeType,
            payload = payload,
            createdAtMs = createdAtMs,
            isFromLocal = false,
        )
        prependHistory(item)
        return item
    }

    fun onNativeHistoryCleared(clearedAtMs: Long) {
        val cur = _config.value
        val effective = maxOf(cur.historyClearedAtMs, clearedAtMs)
        if (effective != cur.historyClearedAtMs) {
            val updated = cur.copy(historyClearedAtMs = effective)
            store.save(updated)
            _config.value = updated
        }
        _history.value = _history.value.filter { it.createdAtMs > effective }
        if (_history.value.isEmpty()) {
            runCatching {
                File(appContext.cacheDir, "clip_share").deleteRecursively()
            }
        }
    }

    fun latestRemoteClip(): UiClipItem? =
        _history.value.firstOrNull { !it.isFromLocal } ?: _history.value.firstOrNull()

    fun clearHistory() {
        val now = System.currentTimeMillis()
        val cur = _config.value
        val updated = cur.copy(historyClearedAtMs = maxOf(cur.historyClearedAtMs, now))
        if (cur.isPaired) {
            store.save(updated)
        }
        _config.value = updated
        _history.value = emptyList()
        // Keep lastSyncedDigest intact so autoSyncOnOpen doesn't re-upload the current Android clipboard.
        BuddyClipNative.nativeClearHistory(updated.historyClearedAtMs)
        runCatching {
            File(appContext.cacheDir, "clip_share").deleteRecursively()
        }
    }

    private fun prependHistory(item: UiClipItem) {
        val current = _history.value
        if (current.any { it.id == item.id }) return
        _history.value = (listOf(item) + current).take(MAX_HISTORY)
    }

    /**
     * Writes a clip to Android's system clipboard (supports UTF-8 text and PNG/JPEG images via FileProvider).
     */
    fun writeClipToSystemClipboard(context: Context, item: UiClipItem): Boolean {
        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return false
        return runCatching {
            if (item.isText) {
                val text = item.textValue ?: return false
                cm.setPrimaryClip(ClipData.newPlainText("buddyclip (${item.originName})", text))
                true
            } else if (item.isImage) {
                val ext = if (item.mimeType.contains("jpeg") || item.mimeType.contains("jpg")) "jpg" else "png"
                val dir = File(context.cacheDir, "clip_share").apply { mkdirs() }
                // Clean old cached clip images so we don't accumulate sensitive screenshots
                dir.listFiles()?.forEach { it.delete() }
                val file = File(dir, "buddyclip_${System.currentTimeMillis()}.$ext")
                file.writeBytes(item.payload)
                val uri = FileProvider.getUriForFile(
                    context,
                    "${context.packageName}.fileprovider",
                    file,
                )
                cm.setPrimaryClip(ClipData.newUri(context.contentResolver, "buddyclip image", uri))
                true
            } else {
                false
            }
        }.onFailure { e ->
            Log.w(TAG, "setPrimaryClip failed (likely OS background restriction): ${e.message}")
        }.getOrDefault(false).also { wrote ->
            // Track what the Android clipboard now holds, so opening the app doesn't send it back.
            if (wrote) lastSyncedDigest = digest(item.mimeType, item.payload)
        }
    }

    /**
     * Reads the Android system clipboard (must be called while an Activity holds window focus),
     * respects Android 13+ EXTRA_IS_SENSITIVE, and pushes over the encrypted Tailnet channel.
     */
    fun readSystemClipboardAndSend(context: Context, skipIfUnchanged: Boolean): SendClipOutcome {
        val cfg = _config.value
        if (!cfg.isPaired) {
            return SendClipOutcome.Failed("Not paired with a PC yet")
        }

        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            ?: return SendClipOutcome.Failed("ClipboardManager unavailable")

        val clip = cm.primaryClip ?: return SendClipOutcome.Empty
        if (clip.itemCount == 0) return SendClipOutcome.Empty

        // Respect password manager sensitivity flags on Android 13+ (API 33+)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val isSensitive = clip.description?.extras?.getBoolean(
                ClipDescription.EXTRA_IS_SENSITIVE,
                false,
            ) == true
            if (isSensitive) {
                return SendClipOutcome.SensitiveSkipped
            }
        }

        val first = clip.getItemAt(0)

        // Check image URI first if present
        val uri = first.uri
        if (uri != null) {
            val mime = context.contentResolver.getType(uri)?.lowercase()
            if (mime != null && (mime == "image/png" || mime == "image/jpeg" || mime == "image/jpg")) {
                return sendImageUri(context, uri, mime, skipIfUnchanged)
            }
        }

        val text = first.coerceToText(context)?.toString()
        if (text.isNullOrEmpty()) return SendClipOutcome.Empty

        return sendDirectText(text, skipIfUnchanged)
    }

    fun sendDirectText(text: String, skipIfUnchanged: Boolean = false): SendClipOutcome {
        val cfg = _config.value
        if (!cfg.isPaired) {
            return SendClipOutcome.Failed("Open buddyclip to pair with your PC first")
        }
        val bytes = text.toByteArray(Charsets.UTF_8)
        if (bytes.isEmpty()) return SendClipOutcome.Empty
        if (bytes.size > MAX_PAYLOAD_BYTES) {
            return SendClipOutcome.Failed("Clip exceeds 16 MiB limit")
        }
        val mime = "text/plain;charset=utf-8"
        val hash = digest(mime, bytes)
        if (skipIfUnchanged && hash == lastSyncedDigest) {
            return SendClipOutcome.Unchanged
        }

        return pushBytesInternal(cfg, mime, bytes, hash)
    }

    fun sendImageUri(
        context: Context,
        uri: Uri,
        explicitMime: String? = null,
        skipIfUnchanged: Boolean = false,
    ): SendClipOutcome {
        val cfg = _config.value
        if (!cfg.isPaired) {
            return SendClipOutcome.Failed("Open buddyclip to pair with your PC first")
        }

        val mime = (explicitMime ?: context.contentResolver.getType(uri) ?: "image/png")
            .lowercase()
            .let { if (it == "image/jpg") "image/jpeg" else it }

        val bytes = runCatching {
            context.contentResolver.openInputStream(uri)?.use { stream ->
                val raw = stream.readBytes()
                if (raw.size > MAX_PAYLOAD_BYTES) null else raw
            }
        }.getOrNull() ?: return SendClipOutcome.Failed("Could not read image (or > 16 MiB)")

        if (bytes.isEmpty()) return SendClipOutcome.Empty

        val hash = digest(mime, bytes)
        if (skipIfUnchanged && hash == lastSyncedDigest) {
            return SendClipOutcome.Unchanged
        }

        return pushBytesInternal(cfg, mime, bytes, hash)
    }

    fun resendClip(item: UiClipItem): SendClipOutcome {
        val cfg = _config.value
        if (!cfg.isPaired) return SendClipOutcome.Failed("Not paired")
        val hash = digest(item.mimeType, item.payload)
        return pushBytesInternal(cfg, item.mimeType, item.payload, hash)
    }

    private fun pushBytesInternal(
        cfg: BuddyClipConfig,
        mime: String,
        bytes: ByteArray,
        hash: String,
    ): SendClipOutcome {
        // Ensure foreground sync engine is running so the Rust socket is alive (or starts & flushes outbox)
        if (!_connection.value.serviceRunning) {
            BuddyClipSyncService.startOrRestart(appContext)
        }

        var err = BuddyClipNative.nativePushClip(mime, bytes)
        if (err != null && err.contains("not running", ignoreCase = true)) {
            // Engine may still be booting on a cold launch from Share Sheet; start directly via service helper
            BuddyClipSyncService.ensureEngineStarted(appContext)
            err = BuddyClipNative.nativePushClip(mime, bytes)
        }

        if (err != null) {
            return SendClipOutcome.Failed(err)
        }

        lastSyncedDigest = hash
        val localItem = UiClipItem(
            id = "local-${System.currentTimeMillis()}",
            originName = cfg.deviceName,
            mimeType = mime,
            payload = bytes,
            createdAtMs = System.currentTimeMillis(),
            isFromLocal = true,
        )
        prependHistory(localItem)
        val target = cfg.hostName.ifBlank { cfg.host }
        return SendClipOutcome.Sent(localItem.preview(48), target)
    }

    private fun digest(mime: String, payload: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        md.update(mime.toByteArray())
        md.update(0)
        md.update(payload)
        return md.digest().joinToString("") { "%02x".format(it) }
    }
}
