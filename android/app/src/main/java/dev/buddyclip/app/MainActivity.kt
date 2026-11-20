package dev.buddyclip.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.os.Build
import android.os.Bundle
import android.text.format.DateUtils
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.ContentPasteGo
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.LinkOff
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.WarningAmber
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private var autoSyncedThisFocus = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        handleDeepLink(intent)

        if (SyncRepository.config.value.isPaired && !SyncRepository.connection.value.serviceRunning) {
            BuddyClipSyncService.startOrRestart(this)
        }

        setContent {
            BuddyClipTheme {
                BuddyClipScreen(
                    onSendClipboard = { sendLocalClipboardNow(skipIfUnchanged = false) },
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleDeepLink(intent)
    }

    override fun onResume() {
        super.onResume()
        autoSyncedThisFocus = false
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || autoSyncedThisFocus) return
        autoSyncedThisFocus = true
        val cfg = SyncRepository.config.value
        if (cfg.isPaired && cfg.autoSyncOnOpen) {
            val outcome = SyncRepository.readSystemClipboardAndSend(this, skipIfUnchanged = true)
            if (outcome is SendClipOutcome.Sent) {
                Toast.makeText(
                    applicationContext,
                    "Auto-synced clipboard to ${outcome.targetName}",
                    Toast.LENGTH_SHORT,
                ).show()
            }
        }
    }

    private fun handleDeepLink(intent: Intent?) {
        val data = intent?.dataString ?: return
        if (!data.startsWith("buddyclip://pair?")) return
        val parsed = BuddyClipNative.parsePairingUri(data)
        if (parsed.ok && parsed.host != null && parsed.key != null) {
            val current = SyncRepository.config.value
            SyncRepository.updateConfig(
                current.copy(
                    host = parsed.host,
                    port = parsed.port ?: 9455,
                    hostName = parsed.hostName ?: parsed.host,
                    keyBase64 = parsed.key,
                ),
            )
            Toast.makeText(
                applicationContext,
                "Paired with ${parsed.hostName ?: parsed.host} (${parsed.fingerprint})",
                Toast.LENGTH_LONG,
            ).show()
        }
    }

    private fun sendLocalClipboardNow(skipIfUnchanged: Boolean): SendClipOutcome =
        SyncRepository.readSystemClipboardAndSend(this, skipIfUnchanged = skipIfUnchanged)
}

@Composable
private fun BuddyClipTheme(content: @Composable () -> Unit) {
    // oddofrancesco.com/design dark palette: pure black (#000000), coral (#D6544B), crisp white (#FFFFFF)
    val scheme = darkColorScheme(
        primary = Color(0xFFD6544B),
        onPrimary = Color(0xFF000000),
        primaryContainer = Color(0xFF2B1412),
        onPrimaryContainer = Color(0xFFD6544B),
        secondary = Color(0xFFD6544B),
        onSecondary = Color(0xFF000000),
        secondaryContainer = Color(0xFF1C1C17),
        onSecondaryContainer = Color(0xFFFFFFFF),
        background = Color(0xFF000000),
        onBackground = Color(0xFFFFFFFF),
        surface = Color(0xFF000000),
        onSurface = Color(0xFFFFFFFF),
        surfaceVariant = Color(0xFF141414),
        onSurfaceVariant = Color(0xFFA3A3A3),
        surfaceContainer = Color(0xFF0E0E0E),
        surfaceContainerHigh = Color(0xFF141414),
        outline = Color(0xFF222222),
        outlineVariant = Color(0xFF222222),
    )
    MaterialTheme(colorScheme = scheme, content = content)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BuddyClipScreen(
    onSendClipboard: () -> SendClipOutcome,
) {
    val context = LocalContext.current
    val config by SyncRepository.config.collectAsState()
    val connection by SyncRepository.connection.collectAsState()
    val history by SyncRepository.history.collectAsState()

    val snackbarHostState = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    var showQrScanner by remember { mutableStateOf(false) }
    var showPairDialog by remember { mutableStateOf(false) }
    var showSettingsDialog by remember { mutableStateOf(false) }

    val notifPermLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) {}

    val cameraPermLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) {
            showQrScanner = true
        } else {
            scope.launch { snackbarHostState.showSnackbar("Camera permission denied — paste the pairing URI instead.") }
        }
    }

    LaunchedEffect(Unit) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val granted = ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
            if (!granted) {
                notifPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
    }

    fun launchScanner() {
        val granted = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.CAMERA,
        ) == PackageManager.PERMISSION_GRANTED
        if (granted) {
            showQrScanner = true
        } else {
            cameraPermLauncher.launch(Manifest.permission.CAMERA)
        }
    }

    fun applyPairingUri(uri: String) {
        val parsed = BuddyClipNative.parsePairingUri(uri.trim())
        if (parsed.ok && parsed.host != null && parsed.key != null) {
            SyncRepository.updateConfig(
                config.copy(
                    host = parsed.host,
                    port = parsed.port ?: 9455,
                    hostName = parsed.hostName ?: parsed.host,
                    keyBase64 = parsed.key,
                ),
            )
            scope.launch {
                snackbarHostState.showSnackbar(
                    "Paired with ${parsed.hostName ?: parsed.host} · Key ${parsed.fingerprint}",
                )
            }
        } else {
            scope.launch {
                snackbarHostState.showSnackbar(parsed.error ?: "Invalid buddyclip pairing URI")
            }
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbarHostState) },
        topBar = {
            TopAppBar(
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(
                            imageVector = Icons.AutoMirrored.Rounded.Send,
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(20.dp),
                        )
                        Spacer(Modifier.width(8.dp))
                        Text(
                            text = "BuddyClip",
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                },
                actions = {
                    if (config.isPaired) {
                        IconButton(onClick = { BuddyClipNative.nativeTriggerReconnect() }) {
                            Icon(Icons.Rounded.Refresh, contentDescription = "Reconnect")
                        }
                    }
                    IconButton(onClick = { showPairDialog = true }) {
                        Icon(Icons.Rounded.QrCodeScanner, contentDescription = "Pair PC")
                    }
                    IconButton(onClick = { showSettingsDialog = true }) {
                        Icon(Icons.Rounded.Settings, contentDescription = "Settings")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = Color.Transparent,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            item {
                ConnectionHeroCard(
                    config = config,
                    connection = connection,
                    onToggleService = { enabled ->
                        if (enabled) BuddyClipSyncService.startOrRestart(context)
                        else BuddyClipSyncService.stop(context)
                    },
                    onScanQr = { launchScanner() },
                    onManualPair = { showPairDialog = true },
                )
            }

            if (config.isPaired) {
                item {
                    QuickActionsRow(
                        hasRemoteClip = SyncRepository.latestRemoteClip() != null,
                        onSend = {
                            val res = onSendClipboard()
                            val msg = when (res) {
                                is SendClipOutcome.Sent -> "Encrypted & sent to ${res.targetName}"
                                SendClipOutcome.Empty -> "Android clipboard is empty"
                                SendClipOutcome.Unchanged -> "Clipboard already synced"
                                SendClipOutcome.SensitiveSkipped -> "Skipped sensitive clipboard item"
                                is SendClipOutcome.Failed -> res.reason
                            }
                            scope.launch { snackbarHostState.showSnackbar(msg) }
                        },
                        onCopyLatest = {
                            val latest = SyncRepository.latestRemoteClip()
                            val msg = if (latest == null) {
                                "No clip received from PC yet"
                            } else if (SyncRepository.writeClipToSystemClipboard(context, latest)) {
                                "Copied clip from ${latest.originName}"
                            } else {
                                "Could not copy to clipboard"
                            }
                            scope.launch { snackbarHostState.showSnackbar(msg) }
                        },
                    )
                }

                if (connection.phase == ConnectionPhase.DISCONNECTED || connection.phase == ConnectionPhase.ERROR) {
                    item {
                        TailnetTroubleshootingCard(port = config.port, detail = connection.detail)
                    }
                }

                item {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = "Live History (RAM-only)",
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                        )
                        if (history.isNotEmpty()) {
                            TextButton(onClick = { SyncRepository.clearHistory() }) {
                                Icon(
                                    Icons.Rounded.DeleteOutline,
                                    contentDescription = null,
                                    modifier = Modifier.size(18.dp),
                                )
                                Spacer(Modifier.width(4.dp))
                                Text("Clear")
                            }
                        }
                    }
                }

                if (history.isEmpty()) {
                    item {
                        Surface(
                            shape = RoundedCornerShape(18.dp),
                            color = MaterialTheme.colorScheme.surfaceContainer,
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Column(
                                modifier = Modifier.padding(24.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                            ) {
                                Text(
                                    text = "Ready for encrypted sync",
                                    style = MaterialTheme.typography.titleSmall,
                                )
                                Spacer(Modifier.height(6.dp))
                                Text(
                                    text = "Copy anything on your PC (Ctrl+C) and it will arrive here automatically over your tailnet.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                    }
                } else {
                    items(history, key = { it.id }) { clip ->
                        ClipHistoryCard(
                            clip = clip,
                            onCopy = {
                                val ok = SyncRepository.writeClipToSystemClipboard(context, clip)
                                scope.launch {
                                    snackbarHostState.showSnackbar(
                                        if (ok) "Copied to clipboard" else "Copy failed",
                                    )
                                }
                            },
                            onResend = {
                                val res = SyncRepository.resendClip(clip)
                                val msg = when (res) {
                                    is SendClipOutcome.Sent -> "Re-sent to ${res.targetName}"
                                    is SendClipOutcome.Failed -> res.reason
                                    else -> "Sent"
                                }
                                scope.launch { snackbarHostState.showSnackbar(msg) }
                            },
                        )
                    }
                }
            }

            item { Spacer(Modifier.height(24.dp)) }
        }
    }

    if (showQrScanner) {
        QrScannerDialog(
            onScanned = { uri ->
                showQrScanner = false
                applyPairingUri(uri)
            },
            onDismiss = { showQrScanner = false },
        )
    }

    if (showPairDialog) {
        PairingDialog(
            initialConfig = config,
            onScanQr = {
                showPairDialog = false
                launchScanner()
            },
            onApplyUri = { uri ->
                showPairDialog = false
                applyPairingUri(uri)
            },
            onSaveManual = { host, port, hostName, keyB64 ->
                showPairDialog = false
                SyncRepository.updateConfig(
                    config.copy(
                        host = host.trim(),
                        port = port,
                        hostName = hostName.trim().ifBlank { host.trim() },
                        keyBase64 = keyB64.trim(),
                    ),
                )
            },
            onDismiss = { showPairDialog = false },
        )
    }

    if (showSettingsDialog) {
        SettingsDialog(
            config = config,
            fingerprint = connection.fingerprint,
            onUpdate = { updated ->
                SyncRepository.updateConfig(updated, restartServiceIfPaired = false)
            },
            onUnpair = {
                showSettingsDialog = false
                SyncRepository.unpair()
                scope.launch { snackbarHostState.showSnackbar("Unpaired and wiped local key") }
            },
            onDismiss = { showSettingsDialog = false },
        )
    }
}

@Composable
private fun ConnectionHeroCard(
    config: BuddyClipConfig,
    connection: ConnectionUiState,
    onToggleService: (Boolean) -> Unit,
    onScanQr: () -> Unit,
    onManualPair: () -> Unit,
) {
    val statusLabel = when (connection.phase) {
        ConnectionPhase.CONNECTED -> "Connected"
        ConnectionPhase.CONNECTING -> "Connecting…"
        ConnectionPhase.DISCONNECTED -> "Reconnecting…"
        ConnectionPhase.ERROR -> "Disconnected"
        ConnectionPhase.STOPPED -> "Paused"
        ConnectionPhase.UNPAIRED -> "Not Paired"
    }

    val peerName = config.hostName.ifBlank { "PC" }
    val subtitle = when {
        !config.isPaired -> "Scan your PC QR code to pair"
        connection.phase == ConnectionPhase.CONNECTED -> peerName
        connection.phase == ConnectionPhase.STOPPED -> peerName
        connection.phase == ConnectionPhase.CONNECTING -> peerName
        else -> "Waiting for $peerName"
    }

    Card(
        shape = RoundedCornerShape(22.dp),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
        ),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(18.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f).padding(end = 12.dp)) {
                    Text(
                        text = statusLabel,
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        text = subtitle,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (config.isPaired) {
                    Switch(
                        checked = connection.serviceRunning,
                        onCheckedChange = onToggleService,
                    )
                }
            }

            if (!config.isPaired) {
                Spacer(Modifier.height(14.dp))
                Text(
                    text = "Run `buddyclip init` on your PC to generate a 256-bit key and QR code. Traffic stays strictly inside your Tailscale tailnet.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(14.dp))
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Button(
                        onClick = onScanQr,
                        modifier = Modifier.weight(1f),
                    ) {
                        Icon(Icons.Rounded.QrCodeScanner, contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Scan QR")
                    }
                    OutlinedButton(
                        onClick = onManualPair,
                        modifier = Modifier.weight(1f),
                    ) {
                        Text("Paste URI")
                    }
                }
            }
        }
    }
}

@Composable
private fun QuickActionsRow(
    hasRemoteClip: Boolean,
    onSend: () -> Unit,
    onCopyLatest: () -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Button(
            onClick = onSend,
            modifier = Modifier
                .weight(1f)
                .height(52.dp),
            shape = RoundedCornerShape(16.dp),
        ) {
            Icon(Icons.AutoMirrored.Rounded.Send, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(8.dp))
            Text("Send Clipboard")
        }
        FilledTonalButton(
            onClick = onCopyLatest,
            enabled = hasRemoteClip,
            modifier = Modifier
                .weight(1f)
                .height(52.dp),
            shape = RoundedCornerShape(16.dp),
        ) {
            Icon(Icons.Rounded.ContentPasteGo, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(8.dp))
            Text("Copy Latest")
        }
    }
}

@Composable
private fun TailnetTroubleshootingCard(port: Int, detail: String) {
    Surface(
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.errorContainer.copy(alpha = 0.35f),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier.padding(14.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Icon(
                Icons.Rounded.WarningAmber,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.error,
                modifier = Modifier.size(20.dp),
            )
            Spacer(Modifier.width(10.dp))
            Column {
                Text(
                    text = "Tailnet connection check",
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    text = "1. Confirm Tailscale VPN is ON on this phone.\n" +
                        "2. Confirm `buddyclip daemon` is running on your PC.\n" +
                        "3. If this phone is a tagged Tailscale device, ensure your ACL allows TCP port $port.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (detail.isNotBlank()) {
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text = detail,
                        style = MaterialTheme.typography.labelSmall,
                        fontFamily = FontFamily.Monospace,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        }
    }
}

@Composable
private fun SeamlessShortcutsTipCard() {
    Surface(
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surfaceContainer,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    Icons.Rounded.Tune,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(Modifier.width(6.dp))
                Text(
                    text = "1-Tap Phone → PC Shortcuts",
                    style = MaterialTheme.typography.labelLarge,
                )
            }
            Spacer(Modifier.height(6.dp))
            Text(
                text = "• Highlight text anywhere → tap \"Send to PC (BuddyClip)\"\n" +
                    "• Share any link or photo → pick BuddyClip\n" +
                    "• Add the \"Send to PC\" Quick Settings tile or tap \"Send Clipboard\" in the notification",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun ClipHistoryCard(
    clip: UiClipItem,
    onCopy: () -> Unit,
    onResend: () -> Unit,
) {
    val relativeTime = remember(clip.createdAtMs) {
        DateUtils.getRelativeTimeSpanString(
            clip.createdAtMs,
            System.currentTimeMillis(),
            DateUtils.SECOND_IN_MILLIS,
            DateUtils.FORMAT_ABBREV_RELATIVE,
        ).toString()
    }

    val imageBitmap = remember(clip.id) {
        if (clip.isImage) {
            runCatching {
                BitmapFactory.decodeByteArray(clip.payload, 0, clip.payload.size)?.asImageBitmap()
            }.getOrNull()
        } else {
            null
        }
    }

    Card(
        shape = RoundedCornerShape(18.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(14.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Surface(
                        shape = RoundedCornerShape(8.dp),
                        color = if (clip.isFromLocal) {
                            MaterialTheme.colorScheme.secondaryContainer
                        } else {
                            MaterialTheme.colorScheme.primaryContainer
                        },
                    ) {
                        Text(
                            text = if (clip.isFromLocal) "Sent · ${clip.originName}" else clip.originName,
                            style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 3.dp),
                        )
                    }
                    Spacer(Modifier.width(8.dp))
                    Text(
                        text = relativeTime,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    FilledTonalIconButton(
                        onClick = onCopy,
                        modifier = Modifier.size(34.dp),
                    ) {
                        Icon(
                            Icons.Rounded.ContentCopy,
                            contentDescription = "Copy",
                            modifier = Modifier.size(16.dp),
                        )
                    }
                    FilledTonalIconButton(
                        onClick = onResend,
                        modifier = Modifier.size(34.dp),
                    ) {
                        Icon(
                            Icons.AutoMirrored.Rounded.Send,
                            contentDescription = "Send to PC",
                            modifier = Modifier.size(16.dp),
                        )
                    }
                }
            }

            Spacer(Modifier.height(10.dp))

            if (imageBitmap != null) {
                Image(
                    bitmap = imageBitmap,
                    contentDescription = "Synced image",
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(160.dp)
                        .clip(RoundedCornerShape(12.dp)),
                )
            } else {
                Text(
                    text = clip.textValue ?: clip.preview(200),
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 4,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
private fun PairingDialog(
    initialConfig: BuddyClipConfig,
    onScanQr: () -> Unit,
    onApplyUri: (String) -> Unit,
    onSaveManual: (host: String, port: Int, hostName: String, keyB64: String) -> Unit,
    onDismiss: () -> Unit,
) {
    var uriText by remember { mutableStateOf("") }
    var host by remember { mutableStateOf(initialConfig.host) }
    var portText by remember { mutableStateOf(initialConfig.port.toString()) }
    var hostName by remember { mutableStateOf(initialConfig.hostName) }
    var keyOrPass by remember { mutableStateOf(initialConfig.keyBase64) }
    var usePassphrase by remember { mutableStateOf(false) }
    var errorMsg by remember { mutableStateOf<String?>(null) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Pair with PC") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Button(
                    onClick = onScanQr,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Icon(Icons.Rounded.QrCodeScanner, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Scan Terminal QR Code")
                }

                OutlinedTextField(
                    value = uriText,
                    onValueChange = {
                        uriText = it
                        errorMsg = null
                    },
                    label = { Text("Or paste buddyclip://pair?... URI") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )

                HorizontalDivider(modifier = Modifier.padding(vertical = 4.dp))

                Text(
                    text = "Manual Tailnet Config",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                OutlinedTextField(
                    value = host,
                    onValueChange = { host = it; errorMsg = null },
                    label = { Text("PC Tailscale IP or .ts.net") },
                    placeholder = { Text("100.64.0.10") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = portText,
                        onValueChange = { portText = it },
                        label = { Text("Port") },
                        singleLine = true,
                        modifier = Modifier.weight(0.4f),
                    )
                    OutlinedTextField(
                        value = hostName,
                        onValueChange = { hostName = it },
                        label = { Text("PC Name") },
                        placeholder = { Text("linux-pc") },
                        singleLine = true,
                        modifier = Modifier.weight(0.6f),
                    )
                }

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Derive key from passphrase", style = MaterialTheme.typography.bodySmall)
                    Switch(checked = usePassphrase, onCheckedChange = { usePassphrase = it })
                }

                OutlinedTextField(
                    value = keyOrPass,
                    onValueChange = { keyOrPass = it; errorMsg = null },
                    label = { Text(if (usePassphrase) "Shared Passphrase (Argon2id)" else "Master Key (base64url)") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )

                if (errorMsg != null) {
                    Text(
                        text = errorMsg!!,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        },
        confirmButton = {
            Button(
                onClick = {
                    if (uriText.isNotBlank()) {
                        onApplyUri(uriText)
                        return@Button
                    }
                    if (!BuddyClipNative.nativeIsTailnetHost(host)) {
                        errorMsg = "Host must be a Tailscale IP (100.64.0.0/10) or *.ts.net"
                        return@Button
                    }
                    val port = portText.toIntOrNull()
                    if (port == null || port !in 1..65535) {
                        errorMsg = "Invalid port"
                        return@Button
                    }
                    val resolvedKey = if (usePassphrase) {
                        val derived = BuddyClipNative.nativeDerivePassphraseKey(keyOrPass)
                        if (derived.startsWith("ERR:")) {
                            errorMsg = derived.removePrefix("ERR:")
                            return@Button
                        }
                        derived
                    } else {
                        if (BuddyClipNative.nativeKeyFingerprint(keyOrPass).isBlank()) {
                            errorMsg = "Invalid 32-byte base64url master key"
                            return@Button
                        }
                        keyOrPass
                    }
                    onSaveManual(host, port, hostName, resolvedKey)
                },
            ) {
                Text("Save & Connect")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel")
            }
        },
    )
}

@Composable
private fun SettingsDialog(
    config: BuddyClipConfig,
    fingerprint: String,
    onUpdate: (BuddyClipConfig) -> Unit,
    onUnpair: () -> Unit,
    onDismiss: () -> Unit,
) {
    var deviceName by remember { mutableStateOf(config.deviceName) }
    var autoSyncOnOpen by remember { mutableStateOf(config.autoSyncOnOpen) }
    var autoWrite by remember { mutableStateOf(config.autoWriteClipboard) }
    var showNotif by remember { mutableStateOf(config.showClipNotification) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Settings") },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedTextField(
                    value = deviceName,
                    onValueChange = { deviceName = it },
                    label = { Text("This device name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                SettingToggleRow(
                    title = "Auto-send clipboard on open",
                    subtitle = "Pushes new clipboard items immediately when you open BuddyClip",
                    checked = autoSyncOnOpen,
                    onCheckedChange = { autoSyncOnOpen = it },
                )
                SettingToggleRow(
                    title = "Auto-copy clips from PC",
                    subtitle = "Immediately writes incoming PC clips to Android clipboard",
                    checked = autoWrite,
                    onCheckedChange = { autoWrite = it },
                )
                SettingToggleRow(
                    title = "Incoming clip notifications",
                    subtitle = "Show subtle silent preview with 1-tap Copy button",
                    checked = showNotif,
                    onCheckedChange = { showNotif = it },
                )
                HorizontalDivider()
                SeamlessShortcutsTipCard()
                if (config.isPaired) {
                    HorizontalDivider()
                    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(
                            text = "Paired PC: ${config.hostName.ifBlank { config.host }} (${config.host}:${config.port})",
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = FontFamily.Monospace,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (fingerprint.isNotBlank()) {
                            Text(
                                text = "Key fingerprint: $fingerprint",
                                style = MaterialTheme.typography.bodySmall,
                                fontFamily = FontFamily.Monospace,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                    OutlinedButton(
                        onClick = onUnpair,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Icon(Icons.Rounded.LinkOff, contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Unpair & Wipe Local Key")
                    }
                }
            }
        },
        confirmButton = {
            Button(
                onClick = {
                    onUpdate(
                        config.copy(
                            deviceName = deviceName.trim().ifBlank { config.deviceName },
                            autoSyncOnOpen = autoSyncOnOpen,
                            autoWriteClipboard = autoWrite,
                            showClipNotification = showNotif,
                        ),
                    )
                    onDismiss()
                },
            ) {
                Text("Done")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel")
            }
        },
    )
}

@Composable
private fun SettingToggleRow(
    title: String,
    subtitle: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, style = MaterialTheme.typography.bodyMedium)
            Text(
                subtitle,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}
