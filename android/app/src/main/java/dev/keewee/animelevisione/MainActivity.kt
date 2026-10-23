package dev.keewee.animelevisione

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.anime_levisione_core.Anime
import uniffi.anime_levisione_core.AnimeLib
import uniffi.anime_levisione_core.Episode

/**
 * Skeleton app: search → episodes → play with ExoPlayer.
 *
 * Every [AnimeLib] call is blocking Rust work (network included); they all
 * run on [Dispatchers.IO] and never on the main thread.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { AnimeApp() }
    }
}

@Composable
private fun AnimeApp() {
    val lib = remember { AnimeLib() }
    DisposableEffect(Unit) { onDispose { lib.close() } }
    val scope = rememberCoroutineScope()

    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf<List<Anime>>(emptyList()) }
    var selected by remember { mutableStateOf<Anime?>(null) }
    var episodes by remember { mutableStateOf<List<Episode>>(emptyList()) }
    var playing by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun search() {
        busy = true
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { lib.search(query) } }
                .onSuccess { results = it; selected = null; error = null }
                .onFailure { error = it.message }
            busy = false
        }
    }

    fun open(anime: Anime) {
        selected = anime
        busy = true
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { lib.episodes(anime) } }
                .onSuccess { episodes = it; error = null }
                .onFailure { error = it.message; selected = null }
            busy = false
        }
    }

    fun play(anime: Anime, episode: Episode) {
        busy = true
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { lib.streamUrl(anime, episode) } }
                .onSuccess { playing = it; error = null }
                .onFailure { error = it.message }
            busy = false
        }
    }

    Scaffold(
        topBar = { TopAppBar(title = { Text("anime-levisione") }) },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                placeholder = { Text("Search — empty lists the latest") },
                singleLine = true,
                trailingIcon = { if (busy) CircularProgressIndicator() },
            )
            error?.let {
                Text(
                    it,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(horizontal = 12.dp),
                )
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                androidx.compose.material3.Button(onClick = { search() }) { Text("Search") }
                if (selected != null) {
                    androidx.compose.material3.OutlinedButton(
                        onClick = { selected = null; playing = null },
                    ) { Text("Back") }
                }
            }
            playing?.let { url -> Player(url = url) } ?: LazyColumn(Modifier.fillMaxSize()) {
                if (selected == null) {
                    items(results) { anime ->
                        TitleRow(anime) { open(anime) }
                    }
                } else {
                    items(episodes) { episode ->
                        EpisodeRow(episode) { play(selected!!, episode) }
                    }
                }
            }
        }
    }
}

@Composable
private fun TitleRow(anime: Anime, onClick: () -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        Text(anime.title, style = MaterialTheme.typography.titleMedium)
        Text(
            listOfNotNull(
                anime.provider.name,
                anime.year.takeIf { it.isNotBlank() },
                anime.kind.takeIf { it.isNotBlank() },
                anime.score.takeIf { it.isNotBlank() },
            ).joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun EpisodeRow(episode: Episode, onClick: () -> Unit) {
    val label = episode.title.takeIf { it.isNotBlank() }
        ?.let { "Ep. ${episode.number} · $it" }
        ?: "Episode ${episode.number}"
    Text(
        label,
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

@Composable
private fun Player(url: String) {
    val context = LocalContext.current
    val player = remember(url) {
        ExoPlayer.Builder(context).build().apply {
            setMediaItem(MediaItem.fromUri(url))
            prepare()
            playWhenReady = true
        }
    }
    DisposableEffect(player) { onDispose { player.release() } }
    AndroidView(
        factory = { context -> PlayerView(context).apply { this.player = player } },
        modifier = Modifier.fillMaxSize(),
    )
}
