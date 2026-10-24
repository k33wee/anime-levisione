@file:androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)

package dev.keewee.animelevisione

import android.app.Activity
import android.content.pm.ActivityInfo
import android.view.ViewGroup
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.TrackGroup
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.Tracks
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.PlayerView
import java.util.Locale
import kotlinx.coroutines.delay
import uniffi.anime_levisione_core.Episode

private data class LanguageTrack(
    val group: TrackGroup,
    val index: Int,
    val language: String,
    val label: String,
)

private fun languageTracks(tracks: Tracks, type: Int): List<LanguageTrack> =
    tracks.groups
        .filter { it.type == type }
        .flatMap { group ->
            (0 until group.length)
                .filter { group.isTrackSupported(it) }
                .map { index ->
                    val format = group.getTrackFormat(index)
                    val language = format.language.orEmpty()
                    val friendly =
                        if (language.isBlank() || language == "und") "Track ${index + 1}"
                        else
                            Locale.forLanguageTag(language)
                                .getDisplayLanguage(Locale.ENGLISH)
                                .replaceFirstChar { it.titlecase() }
                    LanguageTrack(
                        group.mediaTrackGroup,
                        index,
                        language,
                        format.label
                            ?.takeIf { it.isNotBlank() }
                            ?.let { if (it.equals(friendly, true)) it else "$friendly · $it" }
                            ?: friendly,
                    )
                }
        }
        .distinctBy { "${it.language}:${it.label}" }

@Composable
fun PlaybackScreen(
    request: Playback,
    offline: OfflineLibrary,
    preferredLanguage: String,
    autoNext: Boolean,
    onLanguage: (String) -> Unit,
    onProgress: (Double, Double, Boolean, Boolean) -> Unit,
    onBack: () -> Unit,
    onEpisode: (Episode) -> Unit,
) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val activity = context as? Activity
    val latestProgress by rememberUpdatedState(onProgress)
    val latestNext by rememberUpdatedState(onEpisode)
    val player =
        remember(request.session) {
            ExoPlayer.Builder(context)
                .setMediaSourceFactory(
                    DefaultMediaSourceFactory(context)
                        .setDataSourceFactory(offline.httpFactory(request.stream.referer))
                )
                .build()
                .apply {
                    setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(C.USAGE_MEDIA)
                            .setContentType(C.AUDIO_CONTENT_TYPE_MOVIE)
                            .build(),
                        true,
                    )
                    setHandleAudioBecomingNoisy(true)
                    trackSelectionParameters =
                        trackSelectionParameters
                            .buildUpon()
                            .setPreferredAudioLanguage(preferredLanguage)
                            .build()
                    if (request.download != null)
                        setMediaSource(
                            DefaultMediaSourceFactory(context)
                                .setDataSourceFactory(
                                    offline.cacheFactory(request.stream.referer, true)
                                )
                                .createMediaSource(request.download.toMediaItem())
                        )
                    else setMediaItem(request.stream.mediaItem())
                    seekTo((request.resume * 1000).toLong())
                    playWhenReady = false
                    prepare()
                }
        }
    var audio by remember { mutableStateOf<List<LanguageTrack>>(emptyList()) }
    var subtitles by remember { mutableStateOf<List<LanguageTrack>>(emptyList()) }
    var audioDialog by remember { mutableStateOf(false) }
    var subtitleDialog by remember { mutableStateOf(false) }
    var qualityDialog by remember { mutableStateOf(false) }
    var confirmed by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var isPlaying by remember { mutableStateOf(false) }
    var buffer by remember { mutableStateOf(true) }
    var selectedLabel by remember { mutableStateOf("Audio") }
    var seconds by remember { mutableStateOf(request.resume) }
    var duration by remember { mutableStateOf(0.0) }
    var completed by remember { mutableStateOf(request.wasCompleted) }
    var synced by remember { mutableStateOf(request.wasCompleted) }
    // Immersive chrome: taps toggle it, playback auto-hides it, pausing pins it.
    var controlsVisible by remember { mutableStateOf(true) }
    var fullscreen by remember { mutableStateOf(false) }
    var scrubbing by remember { mutableStateOf<Float?>(null) }
    val previousOrientation = remember { activity?.requestedOrientation }
    val index = request.episodes.indexOfFirst { it.id == request.episode.id }
    val next = request.episodes.getOrNull(index + 1).takeIf { index >= 0 }
    val previous = request.episodes.getOrNull(index - 1)
    fun save() {
        val position = player.currentPosition.coerceAtLeast(0) / 1000.0
        val length = player.duration.takeIf { it != C.TIME_UNSET && it > 0 }?.div(1000.0) ?: 0.0
        val done = completed || (length > 0 && position / length >= .9)
        val sync = done && !synced
        if (sync) synced = true
        completed = done
        latestProgress(position, length, done, sync)
    }
    fun togglePlay() {
        if (!confirmed) {
            if (audio.size > 1) audioDialog = true else player.play()
        } else if (player.isPlaying) player.pause() else player.play()
    }
    BackHandler {
        if (fullscreen) fullscreen = false
        else {
            save()
            onBack()
        }
    }
    DisposableEffect(player, lifecycle) {
        val window = activity?.window
        window?.let {
            WindowCompat.getInsetsController(it, it.decorView)
                .hide(WindowInsetsCompat.Type.systemBars())
            it.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
        val listener =
            object : Player.Listener {
                override fun onTracksChanged(tracks: Tracks) {
                    audio = languageTracks(tracks, C.TRACK_TYPE_AUDIO)
                    subtitles = languageTracks(tracks, C.TRACK_TYPE_TEXT)
                    if (!confirmed) {
                        if (audio.size > 1) audioDialog = true
                        else if (audio.size == 1) {
                            selectedLabel = audio.first().label
                            confirmed = true
                            player.play()
                        }
                    }
                }

                override fun onIsPlayingChanged(value: Boolean) {
                    isPlaying = value
                }

                override fun onPlayerError(exception: PlaybackException) {
                    error =
                        "Playback failed (${exception.errorCodeName}). Retry, change provider, or check your connection."
                    buffer = false
                }

                override fun onPlaybackStateChanged(state: Int) {
                    buffer = state == Player.STATE_BUFFERING
                    if (state == Player.STATE_READY && audio.isEmpty() && !confirmed) {
                        confirmed = true
                        player.play()
                    }
                    if (state == Player.STATE_ENDED) {
                        save()
                        if (autoNext && next != null) latestNext(next)
                    }
                }
            }
        player.addListener(listener)
        // Tracks may have arrived between prepare() and installing the listener.
        listener.onTracksChanged(player.currentTracks)
        var resumeOnStart = false
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_STOP -> {
                    resumeOnStart = player.isPlaying
                    player.pause()
                    save()
                }
                Lifecycle.Event.ON_START -> if (resumeOnStart && confirmed) player.play()
                else -> Unit
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            save()
            lifecycle.removeObserver(observer)
            player.removeListener(listener)
            player.release()
            activity?.requestedOrientation =
                previousOrientation ?: ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            window?.let {
                it.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                WindowCompat.getInsetsController(it, it.decorView)
                    .show(WindowInsetsCompat.Type.systemBars())
            }
        }
    }
    // Fullscreen is landscape; leaving restores the orientation the app had.
    LaunchedEffect(fullscreen) {
        activity?.requestedOrientation =
            if (fullscreen) ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            else ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
    }
    // The overlay only fades while actually playing; pause/pickers/errors pin it.
    LaunchedEffect(
        controlsVisible,
        confirmed,
        isPlaying,
        buffer,
        error,
        audioDialog,
        subtitleDialog,
        qualityDialog,
    ) {
        if (
            controlsVisible &&
                confirmed &&
                isPlaying &&
                !buffer &&
                error == null &&
                !audioDialog &&
                !subtitleDialog &&
                !qualityDialog
        ) {
            delay(5000)
            controlsVisible = false
        }
    }
    LaunchedEffect(player) {
        while (true) {
            delay(1000)
            seconds = player.currentPosition.coerceAtLeast(0) / 1000.0
            duration = player.duration.takeIf { it > 0 && it != C.TIME_UNSET }?.div(1000.0) ?: 0.0
            if (isPlaying && seconds.toLong() % 5 == 0L) save()
        }
    }
    val showControls = controlsVisible || !confirmed || error != null
    Box(Modifier.fillMaxSize().background(Color.Black).testTag("playerRoot")) {
        AndroidView(
            factory = {
                PlayerView(it).apply {
                    this.player = player
                    useController = false
                    layoutParams =
                        ViewGroup.LayoutParams(
                            ViewGroup.LayoutParams.MATCH_PARENT,
                            ViewGroup.LayoutParams.MATCH_PARENT,
                        )
                    keepScreenOn = true
                }
            },
            update = { it.player = player },
            modifier = Modifier.fillMaxSize().testTag("videoPlayer"),
        )
        // Taps anywhere on the video toggle the chrome; it never blocks the video itself.
        Box(
            Modifier.matchParentSize()
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                ) {
                    controlsVisible = !controlsVisible
                }
                .testTag("videoSurface")
        )
        if (buffer && error == null) CircularProgressIndicator(Modifier.align(Alignment.Center))
        if (confirmed && !isPlaying && !buffer && error == null)
            IconButton(
                onClick = ::togglePlay,
                modifier =
                    Modifier.align(Alignment.Center)
                        .background(
                            Color.Black.copy(alpha = .45f),
                            androidx.compose.foundation.shape.CircleShape,
                        )
                        .size(84.dp)
                        .testTag("centerPlay"),
            ) {
                Icon(Icons.Default.PlayArrow, "Play", Modifier.size(44.dp), tint = Color.White)
            }
        error?.let {
            Column(
                Modifier.align(Alignment.Center)
                    .background(Panel)
                    .padding(20.dp)
                    .widthIn(max = 420.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(it, color = Color.White)
                TextButton(
                    onClick = {
                        error = null
                        player.prepare()
                        if (confirmed) player.play()
                    }
                ) {
                    Text("Retry playback")
                }
                TextButton(
                    onClick = {
                        save()
                        onBack()
                    }
                ) {
                    Text("Back to episodes")
                }
            }
        }
        if (showControls)
            CompositionLocalProvider(LocalContentColor provides Color.White) {
                Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
                    Box(
                        Modifier.background(
                            Brush.verticalGradient(
                                listOf(Color.Black.copy(alpha = .72f), Color.Transparent)
                            )
                        )
                    ) {
                        Row(
                            Modifier.fillMaxWidth().padding(horizontal = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            IconButton(
                                onClick = {
                                    save()
                                    if (fullscreen) fullscreen = false else onBack()
                                }
                            ) {
                                Icon(Icons.AutoMirrored.Filled.ArrowBack, "Exit player")
                            }
                            Column(Modifier.weight(1f)) {
                                Text(request.anime.title, maxLines = 1, fontSize = 14.sp)
                                Text(
                                    "Episode ${request.episode.number}${if (request.download != null) " • Offline" else " • ${request.anime.provider.label()}"}",
                                    color = Muted,
                                    fontSize = 11.sp,
                                )
                            }
                            IconButton(
                                onClick = {
                                    player.pause()
                                    audioDialog = true
                                },
                                enabled = audio.isNotEmpty(),
                                modifier = Modifier.testTag("audioButton"),
                            ) {
                                Icon(Icons.AutoMirrored.Filled.VolumeUp, "Choose audio language")
                            }
                            IconButton(
                                onClick = { subtitleDialog = true },
                                modifier = Modifier.testTag("subtitleButton"),
                            ) {
                                Icon(Icons.Default.Subtitles, "Choose subtitles")
                            }
                            IconButton(
                                onClick = { qualityDialog = true },
                                enabled = request.download == null,
                                modifier = Modifier.testTag("qualityButton"),
                            ) {
                                Icon(Icons.Default.Tune, "Playback quality")
                            }
                            IconButton(
                                onClick = { fullscreen = !fullscreen },
                                modifier =
                                    Modifier.testTag("toggleFullscreen").semantics {
                                        contentDescription =
                                            if (fullscreen) "Exit fullscreen"
                                            else "Enter fullscreen"
                                    },
                            ) {
                                Icon(
                                    if (fullscreen) Icons.Default.FullscreenExit
                                    else Icons.Default.Fullscreen,
                                    if (fullscreen) "Exit fullscreen" else "Enter fullscreen",
                                )
                            }
                        }
                    }
                    Spacer(Modifier.weight(1f))
                    Box(
                        Modifier.background(
                            Brush.verticalGradient(
                                listOf(Color.Transparent, Color.Black.copy(alpha = .78f))
                            )
                        )
                    ) {
                        Column(
                            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)
                        ) {
                            Text(
                                "${formatTime(scrubbing?.times(duration) ?: seconds)} / ${formatTime(duration)} • ${if (isPlaying) "Playing" else if (!confirmed) "Choose audio to start" else "Paused"}",
                                modifier = Modifier.testTag("playbackStatus"),
                                color = Color.White,
                                fontSize = 12.sp,
                            )
                            Slider(
                                value =
                                    scrubbing
                                        ?: if (duration > 0)
                                            (seconds / duration).toFloat().coerceIn(0f, 1f)
                                        else 0f,
                                onValueChange = { scrubbing = it },
                                onValueChangeFinished = {
                                    scrubbing?.let {
                                        player.seekTo((it * duration * 1000).toLong())
                                    }
                                    scrubbing = null
                                },
                                enabled = duration > 0,
                                colors =
                                    SliderDefaults.colors(
                                        activeTrackColor = Accent,
                                        thumbColor = Accent,
                                        inactiveTrackColor = Color.White.copy(alpha = .32f),
                                    ),
                                modifier = Modifier.fillMaxWidth().testTag("positionSlider"),
                            )
                            Row(
                                Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceEvenly,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                IconButton(
                                    onClick = {
                                        previous?.let {
                                            save()
                                            onEpisode(it)
                                        }
                                    },
                                    enabled = previous != null,
                                ) {
                                    Icon(Icons.Default.SkipPrevious, "Previous episode")
                                }
                                IconButton(
                                    onClick = {
                                        player.seekTo(0)
                                        if (confirmed) player.play()
                                    }
                                ) {
                                    Icon(Icons.Default.Replay, "Restart episode")
                                }
                                IconButton(
                                    onClick = ::togglePlay,
                                    modifier = Modifier.testTag("playPause"),
                                ) {
                                    Icon(
                                        if (isPlaying) Icons.Default.Pause
                                        else Icons.Default.PlayArrow,
                                        if (isPlaying) "Pause" else "Play",
                                        tint = Color.White,
                                    )
                                }
                                IconButton(
                                    onClick = {
                                        next?.let {
                                            save()
                                            onEpisode(it)
                                        }
                                    },
                                    enabled = next != null,
                                ) {
                                    Icon(Icons.Default.SkipNext, "Next episode")
                                }
                            }
                        }
                    }
                }
            }
    }
    if (audioDialog)
        AlertDialog(
            onDismissRequest = {
                if (confirmed) {
                    audioDialog = false
                    player.play()
                } else {
                    save()
                    onBack()
                }
            },
            title = { Text("Choose audio") },
            text = {
                Column {
                    Text("Pick the language you want to hear.", color = Muted)
                    LazyColumn {
                        items(audio) { track ->
                            Row(
                                Modifier.fillMaxWidth()
                                    .clickable {
                                        player.trackSelectionParameters =
                                            player.trackSelectionParameters
                                                .buildUpon()
                                                .setOverrideForType(
                                                    TrackSelectionOverride(track.group, track.index)
                                                )
                                                .build()
                                        selectedLabel = track.label
                                        if (track.language.isNotBlank()) onLanguage(track.language)
                                        confirmed = true
                                        audioDialog = false
                                        player.play()
                                    }
                                    .padding(vertical = 16.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Icon(Icons.AutoMirrored.Filled.VolumeUp, null, tint = Accent)
                                Text(track.label, Modifier.padding(start = 12.dp))
                            }
                        }
                    }
                }
            },
            confirmButton = {},
            dismissButton = {
                TextButton(
                    onClick = {
                        if (confirmed) {
                            audioDialog = false
                            player.play()
                        } else {
                            save()
                            onBack()
                        }
                    }
                ) {
                    Text(if (confirmed) "Close" else "Cancel playback")
                }
            },
        )
    if (subtitleDialog)
        AlertDialog(
            onDismissRequest = { subtitleDialog = false },
            title = { Text("Subtitles") },
            text = {
                LazyColumn {
                    item {
                        Text(
                            "Off",
                            Modifier.fillMaxWidth()
                                .clickable {
                                    player.trackSelectionParameters =
                                        player.trackSelectionParameters
                                            .buildUpon()
                                            .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
                                            .build()
                                    subtitleDialog = false
                                }
                                .padding(16.dp),
                        )
                    }
                    items(subtitles) { track ->
                        Text(
                            track.label,
                            Modifier.fillMaxWidth()
                                .clickable {
                                    player.trackSelectionParameters =
                                        player.trackSelectionParameters
                                            .buildUpon()
                                            .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, false)
                                            .setOverrideForType(
                                                TrackSelectionOverride(track.group, track.index)
                                            )
                                            .build()
                                    subtitleDialog = false
                                }
                                .padding(16.dp),
                        )
                    }
                    if (subtitles.isEmpty())
                        item {
                            Text(
                                "No separate subtitle tracks. Subtitles may be part of the video.",
                                color = Muted,
                                modifier = Modifier.padding(16.dp),
                            )
                        }
                }
            },
            confirmButton = { TextButton(onClick = { subtitleDialog = false }) { Text("Close") } },
        )
    if (qualityDialog)
        AlertDialog(
            onDismissRequest = { qualityDialog = false },
            title = { Text("Playback quality") },
            text = {
                Column {
                    listOf(0, 480, 720, 1080).forEach { height ->
                        Text(
                            if (height == 0) "Auto • adapt to your connection"
                            else "Up to ${height}p",
                            Modifier.fillMaxWidth()
                                .clickable {
                                    player.trackSelectionParameters =
                                        player.trackSelectionParameters
                                            .buildUpon()
                                            .setMaxVideoSize(
                                                if (height == 0) Int.MAX_VALUE else height * 16 / 9,
                                                if (height == 0) Int.MAX_VALUE else height,
                                            )
                                            .build()
                                    qualityDialog = false
                                }
                                .padding(14.dp),
                        )
                    }
                }
            },
            confirmButton = { TextButton(onClick = { qualityDialog = false }) { Text("Close") } },
        )
}
