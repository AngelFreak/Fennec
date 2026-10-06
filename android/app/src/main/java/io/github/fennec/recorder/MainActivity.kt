package io.github.fennec.recorder

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaPlayer
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.record.RecordRequest
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.ui.DetailScreen
import io.github.fennec.recorder.ui.FennecIcon
import io.github.fennec.recorder.ui.MainViewModel
import io.github.fennec.recorder.ui.PairScreen
import io.github.fennec.recorder.ui.PairUi
import io.github.fennec.recorder.ui.Playback
import io.github.fennec.recorder.ui.RecordScreen
import io.github.fennec.recorder.ui.RecordingsScreen
import io.github.fennec.recorder.ui.Screen
import io.github.fennec.recorder.ui.SettingsScreen
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.FennecTheme
import kotlinx.coroutines.delay
import java.io.File

class MainActivity : ComponentActivity() {
    private val vm: MainViewModel by viewModels()

    /** Set by the tile or widget: start recording once the screen is up. */
    private var recordNow by mutableStateOf(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        handle(intent)
        setContent { FennecTheme { App(vm, recordNow) { recordNow = false } } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent?) {
        val data = intent?.dataString
        if (intent?.action == Intent.ACTION_VIEW && data != null) vm.pairWith(data, fromLink = true)
        if (intent?.action == ACTION_RECORD) recordNow = true
    }

    companion object {
        const val ACTION_RECORD = "io.github.fennec.recorder.RECORD_NOW"
    }
}

@Composable
private fun App(vm: MainViewModel, recordNow: Boolean, onRecordStarted: () -> Unit) {
    val context = LocalContext.current
    val c = Fennec.colors
    val screen by vm.screen.collectAsState()
    val recordings by vm.recordings.collectAsState()
    val paired by vm.app.pairing.paired.collectAsState()
    val info by vm.app.pairing.info.collectAsState()
    val contact by vm.app.lastContact.collectAsState()
    val settings by vm.app.settings.settings.collectAsState()
    val status by Recorder.state.collectAsState()
    val next by vm.next.collectAsState()
    val pairUi by vm.pair.collectAsState()

    val startRecording = {
        val n = vm.next.value
        Recorder.start(
            context,
            RecordRequest(n.title, n.project?.id, n.project?.name, n.template?.id, n.template?.name),
        )
        vm.setNext(n.copy(title = ""))
    }
    val micAllowed = {
        ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
    }
    // The notification permission is asked with the microphone; only the microphone is needed.
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        if (micAllowed()) startRecording()
    }
    val record = {
        val needed = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }.filter { ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED }
        if (needed.isEmpty()) startRecording() else askMic.launch(needed.toTypedArray())
    }
    LaunchedEffect(recordNow) {
        if (recordNow && status !is Recorder.Status.Recording) {
            vm.go(Screen.Record)
            record()
        }
        if (recordNow) onRecordStarted()
    }
    // Follow Fennec while the app is open.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        var job: kotlinx.coroutines.Job? = null
        val observer = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_START) job = vm.watch()
            if (e == Lifecycle.Event.ON_STOP) job?.cancel()
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            job?.cancel()
        }
    }
    BackHandler(enabled = screen != Screen.Record) { vm.back() }

    Column(Modifier.fillMaxSize().background(c.surface).statusBarsPadding()) {
        Box(Modifier.weight(1f)) {
            when (val s = screen) {
                Screen.Record -> RecordScreen(
                    status, next, info, paired?.name, contact.reachable.takeIf { paired != null },
                    onNext = vm::setNext, onRecord = record,
                    onPause = { Recorder.pause(context) }, onResume = { Recorder.resume(context) },
                    onStop = { Recorder.stop(context) },
                    onPair = { vm.go(Screen.Pair) }, onSettings = { vm.go(Screen.Settings) },
                )
                Screen.Recordings -> RecordingsScreen(
                    recordings, info.projects.associateBy { it.id }, paired?.name, contact.reachable, contact.at,
                    onOpen = { vm.go(Screen.Detail(it)) }, onPair = { vm.go(Screen.Pair) },
                    onSettings = { vm.go(Screen.Settings) },
                )
                is Screen.Detail -> {
                    val r = recordings.firstOrNull { it.id == s.id }
                    if (r == null) {
                        LaunchedEffect(Unit) { vm.go(Screen.Recordings) }
                    } else {
                        val player = rememberPlayer(File(vm.app.dir, r.fileName), r)
                        DetailScreen(
                            r, info, paired?.name, player.state, onBack = { vm.back() }, onPlay = player.toggle,
                            onRename = { vm.rename(r.id, it) }, onProject = { vm.setProject(r.id, it) },
                            onTemplate = { vm.setTemplate(r.id, it) }, onRetry = { vm.retry(r.id) },
                            onDelete = { vm.delete(r.id) },
                        )
                    }
                }
                Screen.Pair -> PairScreen(
                    pairUi,
                    camera = { Camera { vm.pairWith(it, fromLink = false) } },
                    onManual = { vm.pair.value = PairUi.Manual() },
                    onSubmitManual = vm::pairManually,
                    onConfirm = vm::startPairing,
                    onRetry = { vm.pair.value = PairUi.Scan },
                    onClose = {
                        vm.cancelPairing()
                        vm.go(Screen.Record)
                    },
                )
                Screen.Settings -> SettingsScreen(
                    paired, settings, BuildConfig.VERSION_NAME, onBack = { vm.back() }, onPair = { vm.go(Screen.Pair) },
                    onUnpair = vm::unpair, onChange = { s2 -> vm.app.settings.update { s2 } },
                )
            }
        }
        if (screen == Screen.Record || screen == Screen.Recordings) {
            HorizontalDivider(color = c.divider)
            NavigationBar(containerColor = c.chrome, modifier = Modifier.navigationBarsPadding(), windowInsets = androidx.compose.foundation.layout.WindowInsets(0)) {
                val colors = NavigationBarItemDefaults.colors(
                    selectedIconColor = c.accentText, selectedTextColor = c.accentText, indicatorColor = c.accentSoft,
                    unselectedIconColor = c.muted, unselectedTextColor = c.muted,
                )
                NavigationBarItem(
                    selected = screen == Screen.Record, onClick = { vm.go(Screen.Record) },
                    icon = { FennecIcon(R.drawable.ic_mic, if (screen == Screen.Record) c.accentText else c.muted) },
                    label = { Text("Record") }, colors = colors,
                )
                NavigationBarItem(
                    selected = screen == Screen.Recordings, onClick = { vm.go(Screen.Recordings) },
                    icon = { FennecIcon(R.drawable.ic_list, if (screen == Screen.Recordings) c.accentText else c.muted) },
                    label = { Text("Recordings") }, colors = colors,
                )
            }
        } else {
            Box(Modifier.navigationBarsPadding())
        }
    }
}

/** The camera, reading QR codes, while camera permission is given. */
@Composable
private fun Camera(onCode: (String) -> Unit) {
    val context = LocalContext.current
    var allowed by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed = it }
    if (!allowed) {
        TextButton({ ask.launch(Manifest.permission.CAMERA) }) {
            Text("Allow the camera to scan the code", color = androidx.compose.ui.graphics.Color(0xFFF59A6B))
        }
        return
    }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var handled by remember { mutableStateOf(false) }
    val view = remember {
        BarcodeView(context).apply {
            decoderFactory = DefaultDecoderFactory(listOf(BarcodeFormat.QR_CODE))
            decodeContinuous { result ->
                val text = result.text ?: return@decodeContinuous
                if (!handled && text.startsWith("fennec://")) {
                    handled = true
                    onCode(text)
                }
            }
        }
    }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) view.resume()
            if (e == Lifecycle.Event.ON_PAUSE) view.pause()
        }
        lifecycle.addObserver(observer)
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) view.resume()
        onDispose {
            lifecycle.removeObserver(observer)
            view.pause()
        }
    }
    AndroidView({ view }, Modifier.fillMaxSize())
}

private class Player(val state: Playback, val toggle: () -> Unit)

@Composable
private fun rememberPlayer(file: File, r: Recording): Player {
    var playback by remember(r.id) { mutableStateOf(Playback()) }
    val player = remember(r.id) { arrayOfNulls<MediaPlayer>(1) }
    DisposableEffect(r.id) {
        onDispose {
            player[0]?.release()
            player[0] = null
        }
    }
    LaunchedEffect(playback.playing) {
        while (playback.playing) {
            val mp = player[0] ?: break
            playback = playback.copy(positionMs = mp.currentPosition.toLong())
            delay(200)
        }
    }
    return Player(playback) {
        val mp = player[0]
        when {
            mp == null && file.exists() && r.state != io.github.fennec.recorder.data.SyncState.RECORDING -> {
                runCatching {
                    MediaPlayer().apply {
                        setDataSource(file.absolutePath)
                        prepare()
                        setOnCompletionListener { playback = Playback(false, 0); it.seekTo(0) }
                        start()
                    }
                }.onSuccess {
                    player[0] = it
                    playback = playback.copy(playing = true)
                }
            }
            mp != null && mp.isPlaying -> {
                mp.pause()
                playback = playback.copy(playing = false)
            }
            mp != null -> {
                mp.start()
                playback = playback.copy(playing = true)
            }
        }
    }
}
