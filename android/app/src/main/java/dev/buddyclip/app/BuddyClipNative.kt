package dev.buddyclip.app

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

interface BuddyClipNativeCallback {
    fun onStateChanged(state: String, detail: String, fingerprint: String)
    fun onClipReceived(
        id: String,
        originName: String,
        mimeType: String,
        payload: ByteArray,
        createdAtMs: Long,
    )
    fun onClipSent(clipId: String)
}

@Serializable
data class ParsedPairingResult(
    val ok: Boolean,
    val host: String? = null,
    val port: Int? = null,
    val key: String? = null,
    val hostName: String? = null,
    val fingerprint: String? = null,
    val error: String? = null,
)

object BuddyClipNative {
    private val json = Json { ignoreUnknownKeys = true }

    init {
        System.loadLibrary("buddyclip_android")
    }

    external fun nativeStart(
        host: String,
        port: Int,
        keyBase64: String,
        deviceId: String,
        deviceName: String,
        callback: BuddyClipNativeCallback,
    ): String?

    external fun nativeStop()

    external fun nativePushClip(mimeType: String, payload: ByteArray): String?

    external fun nativeTriggerReconnect()

    external fun nativeRequestSync()

    external fun nativeParsePairingUri(uri: String): String

    external fun nativeKeyFingerprint(keyBase64: String): String

    external fun nativeDerivePassphraseKey(passphrase: String): String

    external fun nativeIsTailnetHost(host: String): Boolean

    fun parsePairingUri(uri: String): ParsedPairingResult {
        val raw = nativeParsePairingUri(uri)
        return runCatching { json.decodeFromString<ParsedPairingResult>(raw) }
            .getOrElse { ParsedPairingResult(ok = false, error = "Invalid pairing response") }
    }
}
