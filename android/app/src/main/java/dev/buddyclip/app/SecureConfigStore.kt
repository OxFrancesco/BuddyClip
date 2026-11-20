package dev.buddyclip.app

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@Serializable
data class BuddyClipConfig(
    val host: String = "",
    val port: Int = 9455,
    val hostName: String = "",
    val keyBase64: String = "",
    val deviceId: String = UUID.randomUUID().toString(),
    val deviceName: String = defaultAndroidDeviceName(),
    val autoSyncOnOpen: Boolean = true,
    val autoWriteClipboard: Boolean = true,
    val showClipNotification: Boolean = true,
    val historyClearedAtMs: Long = 0L,
) {
    val isPaired: Boolean
        get() = host.isNotBlank() && keyBase64.isNotBlank() && port in 1..65535
}

private fun defaultAndroidDeviceName(): String {
    val model = Build.MODEL?.trim().orEmpty()
    val brand = Build.MANUFACTURER?.trim().orEmpty()
    return when {
        model.isBlank() -> "Android"
        brand.isNotBlank() && !model.startsWith(brand, ignoreCase = true) ->
            "${brand.replaceFirstChar { it.uppercase() }} $model"
        else -> model
    }
}

/**
 * Hardware-backed Android Keystore (AES-256-GCM) config store.
 * Keeps the 256-bit shared clipboard key encrypted at rest inside app-private storage.
 */
class SecureConfigStore(context: Context) {
    private val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
    private val json = Json { ignoreUnknownKeys = true; encodeDefaults = true }

    fun load(): BuddyClipConfig {
        val blob = prefs.getString(KEY_BLOB, null) ?: return BuddyClipConfig()
        return runCatching {
            val plaintext = decrypt(blob)
            json.decodeFromString<BuddyClipConfig>(plaintext)
        }.getOrElse {
            BuddyClipConfig()
        }
    }

    fun save(config: BuddyClipConfig) {
        val plaintext = json.encodeToString(config)
        val encrypted = encrypt(plaintext)
        prefs.edit().putString(KEY_BLOB, encrypted).apply()
    }

    fun clear() {
        prefs.edit().remove(KEY_BLOB).apply()
    }

    private fun getOrCreateKey(): SecretKey {
        val ks = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (ks.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }

        val keyGen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        val spec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        keyGen.init(spec)
        return keyGen.generateKey()
    }

    private fun encrypt(plaintext: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
        val iv = cipher.iv
        val ct = cipher.doFinal(plaintext.toByteArray(Charsets.UTF_8))
        val combined = ByteArray(1 + iv.size + ct.size)
        combined[0] = iv.size.toByte()
        System.arraycopy(iv, 0, combined, 1, iv.size)
        System.arraycopy(ct, 0, combined, 1 + iv.size, ct.size)
        return Base64.encodeToString(combined, Base64.NO_WRAP)
    }

    private fun decrypt(encoded: String): String {
        val combined = Base64.decode(encoded, Base64.NO_WRAP)
        val ivLen = combined[0].toInt() and 0xFF
        val iv = combined.copyOfRange(1, 1 + ivLen)
        val ct = combined.copyOfRange(1 + ivLen, combined.size)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, getOrCreateKey(), GCMParameterSpec(128, iv))
        val pt = cipher.doFinal(ct)
        return String(pt, Charsets.UTF_8)
    }

    companion object {
        private const val PREFS_NAME = "buddyclip_secure_cfg"
        private const val KEY_BLOB = "config_v1"
        private const val ANDROID_KEYSTORE = "AndroidKeyStore"
        private const val KEY_ALIAS = "buddyclip_master_wrap_v1"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
