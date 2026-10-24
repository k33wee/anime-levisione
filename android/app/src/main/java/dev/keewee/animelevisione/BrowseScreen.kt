@file:OptIn(
    androidx.compose.material3.ExperimentalMaterial3Api::class,
    androidx.compose.foundation.layout.ExperimentalLayoutApi::class,
)
@file:androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)

package dev.keewee.animelevisione

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.media3.exoplayer.offline.Download
import coil.compose.AsyncImage
import kotlinx.coroutines.launch
import uniffi.anime_levisione_core.*

@Composable
fun LevisioneApp(vm: CatalogViewModel = viewModel()) {
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(vm.message) {
        vm.message?.let { message ->
            vm.message = null
            snackbar.showSnackbar(message.take(500))
        }
    }
    val playing = vm.playback
    if (playing != null) {
        Box(Modifier.fillMaxSize()) {
            key(playing.session) {
                PlaybackScreen(
                    playing,
                    vm.offline,
                    vm.audioLanguage,
                    vm.autoNext,
                    onLanguage = { vm.preferences(audio = it) },
                    onProgress = { seconds, duration, done, sync ->
                        vm.persist(playing, seconds, duration, done, sync)
                    },
                    onBack = vm::stopPlayback,
                    onEpisode = { vm.play(playing.anime, it, playing.episodes) },
                )
            }
            SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
            if (vm.resolving)
                AlertDialog(
                    onDismissRequest = vm::cancelResolve,
                    title = { Text("Loading the next episode") },
                    text = { LinearProgressIndicator(Modifier.fillMaxWidth()) },
                    confirmButton = { TextButton(onClick = vm::cancelResolve) { Text("Cancel") } },
                )
        }
        return
    }
    BackHandler(vm.detail != null || vm.destination != Destination.Home || vm.resolving) {
        when {
            vm.resolving -> vm.cancelResolve()
            vm.detail != null -> vm.closeDetail()
            else -> vm.destination = Destination.Home
        }
    }
    Scaffold(
        containerColor = Ink,
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            if (vm.detail == null)
                NavigationBar(containerColor = Ink, tonalElevation = 0.dp) {
                    listOf(
                            Destination.Home to Icons.Default.Home,
                            Destination.Search to Icons.Default.Search,
                            Destination.Library to Icons.Default.Bookmarks,
                            Destination.Downloads to Icons.Default.Download,
                            Destination.Settings to Icons.Default.Person,
                        )
                        .forEach { (tab, icon) ->
                            NavigationBarItem(
                                selected = vm.destination == tab,
                                onClick = { vm.destination = tab },
                                modifier = Modifier.testTag("nav${tab.name}"),
                                icon = { Icon(icon, null) },
                                label = {
                                    Text(
                                        if (tab == Destination.Library) "My List" else tab.name,
                                        fontSize = 10.sp,
                                    )
                                },
                                colors =
                                    NavigationBarItemDefaults.colors(
                                        indicatorColor = Accent.copy(alpha = .16f),
                                        selectedIconColor = Accent,
                                        selectedTextColor = Color.White,
                                        unselectedIconColor = Muted,
                                        unselectedTextColor = Muted,
                                    ),
                            )
                        }
                }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            val detail = vm.detail
            if (detail != null) DetailScreen(vm, detail)
            else
                Column {
                    BrandHeader(vm.destination, vm.settings?.privateMode == true) {
                        vm.destination = Destination.Search
                    }
                    when (vm.destination) {
                        Destination.Home -> HomeScreen(vm)
                        Destination.Search -> SearchScreen(vm)
                        Destination.Library -> LibraryScreen(vm)
                        Destination.Downloads -> DownloadsScreen(vm)
                        Destination.Settings -> SettingsScreen(vm)
                    }
                }
            if (vm.resolving)
                AlertDialog(
                    onDismissRequest = vm::cancelResolve,
                    title = { Text("Getting your episode ready") },
                    text = {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            CircularProgressIndicator(Modifier.size(24.dp))
                            Spacer(Modifier.width(16.dp))
                            Text("Resolving the best stream…")
                        }
                    },
                    confirmButton = { TextButton(onClick = vm::cancelResolve) { Text("Cancel") } },
                )
        }
    }
}

@Composable
private fun BrandHeader(destination: Destination, isPrivate: Boolean, search: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(5.dp, 24.dp).clip(RoundedCornerShape(3.dp)).background(Accent))
        Spacer(Modifier.width(9.dp))
        Text("levisione", fontSize = 25.sp, fontWeight = FontWeight.Black, letterSpacing = (-1).sp)
        Spacer(Modifier.weight(1f))
        if (isPrivate)
            AssistChip(
                onClick = {},
                label = { Text("Private", fontSize = 11.sp) },
                leadingIcon = { Icon(Icons.Default.VisibilityOff, null, Modifier.size(14.dp)) },
            )
        if (destination != Destination.Search)
            IconButton(onClick = search) { Icon(Icons.Default.Search, "Search catalogue") }
    }
}

@Composable
private fun HomeScreen(vm: CatalogViewModel) {
    val rows = vm.catalogue
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(24.dp),
        contentPadding = PaddingValues(bottom = 24.dp),
    ) {
        if (rows.isNotEmpty())
            item {
                val hero = rows.firstOrNull { it.coverUrl.isNotBlank() } ?: rows.first()
                Box(Modifier.fillMaxWidth().height(390.dp).testTag("homeHero")) {
                    Poster(hero, Modifier.fillMaxSize(), ContentScale.Crop)
                    Box(
                        Modifier.fillMaxSize()
                            .background(
                                Brush.verticalGradient(
                                    listOf(Ink.copy(alpha = .1f), Ink.copy(alpha = .3f), Ink)
                                )
                            )
                    )
                    Column(Modifier.align(Alignment.BottomStart).padding(24.dp)) {
                        Text(
                            "YOUR NEXT OBSESSION",
                            color = Accent,
                            style = MaterialTheme.typography.labelMedium,
                            letterSpacing = 2.sp,
                        )
                        Spacer(Modifier.height(10.dp))
                        Text(
                            hero.title,
                            fontSize = 32.sp,
                            fontWeight = FontWeight.Black,
                            lineHeight = 35.sp,
                            maxLines = 3,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Spacer(Modifier.height(8.dp))
                        Text(
                            "${hero.provider.label()}  •  ${hero.audioLabel()}",
                            color = Muted,
                            style = MaterialTheme.typography.labelMedium,
                        )
                        Spacer(Modifier.height(18.dp))
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            Button(onClick = { vm.open(hero) }, shape = RoundedCornerShape(10.dp)) {
                                Icon(Icons.Default.PlayArrow, null)
                                Text("Explore episodes", Modifier.padding(start = 4.dp))
                            }
                            FilledTonalIconButton(onClick = { vm.toggleWatchlist(hero) }) {
                                Icon(
                                    if (vm.library.watchlist.any { it.key() == hero.key() })
                                        Icons.Default.Check
                                    else Icons.Default.Add,
                                    "My List",
                                )
                            }
                        }
                    }
                }
            }
        if (vm.catalogueLoading && rows.isEmpty()) item { LoadingBlock("Finding fresh episodes…") }
        vm.catalogueError?.let {
            item { ErrorBlock("The catalogue couldn’t load", it, vm::loadCatalogue) }
        }
        if (vm.library.resume.isNotEmpty()) item { ContinueRow(vm) }
        if (vm.library.watchlist.isNotEmpty())
            item {
                PosterRail("Your list", "Saved for a good night in", vm.library.watchlist, vm::open)
            }
        ProviderId.values().forEach { provider ->
            val titles = rows.filter { it.provider == provider }
            if (titles.isNotEmpty())
                item {
                    PosterRail(
                        "Fresh on ${provider.label()}",
                        "New episodes. Less browsing.",
                        titles,
                        vm::open,
                    )
                }
        }
        item {
            TextButton(
                onClick = vm::loadCatalogue,
                modifier = Modifier.padding(horizontal = 12.dp),
                enabled = !vm.catalogueLoading,
            ) {
                Icon(Icons.Default.Refresh, null)
                Text("Refresh releases", Modifier.padding(start = 8.dp))
            }
        }
    }
}

@Composable
private fun PosterRail(
    title: String,
    subtitle: String,
    titles: List<Anime>,
    open: (Anime) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionHeading(title, subtitle)
        LazyRow(
            contentPadding = PaddingValues(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            items(titles, key = { it.key() }) { anime ->
                PosterCard(anime, Modifier.width(132.dp)) { open(anime) }
            }
        }
    }
}

@Composable
private fun SectionHeading(title: String, subtitle: String = "") {
    Column(Modifier.padding(horizontal = 20.dp)) {
        Text(title, fontSize = 21.sp, fontWeight = FontWeight.Bold)
        if (subtitle.isNotBlank())
            Text(
                subtitle,
                color = Muted,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(top = 4.dp),
            )
    }
}

@Composable
fun Poster(anime: Anime, modifier: Modifier = Modifier, scale: ContentScale = ContentScale.Crop) {
    Box(
        modifier.background(Brush.linearGradient(listOf(Color(0xFF29334B), Color(0xFF24172D)))),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            anime.title.take(1).uppercase(),
            color = Color.White.copy(alpha = .12f),
            fontSize = 64.sp,
            fontWeight = FontWeight.Black,
        )
        if (anime.coverUrl.isNotBlank())
            AsyncImage(anime.coverUrl, null, Modifier.fillMaxSize(), contentScale = scale)
    }
}

@Composable
private fun PosterCard(anime: Anime, modifier: Modifier = Modifier, open: () -> Unit) {
    Column(modifier.clickable(onClickLabel = "Details for ${anime.title}", onClick = open)) {
        Box(Modifier.fillMaxWidth().aspectRatio(.68f).clip(RoundedCornerShape(10.dp))) {
            Poster(anime, Modifier.fillMaxSize())
            Surface(
                color = Ink.copy(alpha = .84f),
                shape = RoundedCornerShape(4.dp),
                modifier = Modifier.align(Alignment.BottomStart).padding(6.dp),
            ) {
                Text(
                    anime.audioLabel(),
                    fontSize = 9.sp,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier.padding(horizontal = 6.dp, vertical = 3.dp),
                )
            }
        }
        Text(
            anime.title,
            fontWeight = FontWeight.SemiBold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            fontSize = 13.sp,
            lineHeight = 17.sp,
            modifier = Modifier.padding(top = 8.dp),
        )
        Text(
            listOf(anime.year, anime.kind)
                .filter { it.isNotBlank() }
                .joinToString(" • ")
                .ifBlank { anime.provider.label() },
            color = Muted,
            fontSize = 11.sp,
            maxLines = 1,
        )
    }
}

@Composable
private fun ContinueRow(vm: CatalogViewModel) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionHeading("Pick up where you left off", "Your next scene is waiting")
        LazyRow(
            contentPadding = PaddingValues(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            items(vm.library.resume, key = { it.anime.key() }) { point ->
                Column(
                    Modifier.width(240.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(Panel)
                        .clickable { vm.play(point.anime, point.episode) }
                ) {
                    Box(Modifier.fillMaxWidth().height(130.dp)) {
                        Poster(point.anime, Modifier.fillMaxSize())
                        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = .25f)))
                        Icon(
                            Icons.Default.PlayCircle,
                            "Resume ${point.anime.title}",
                            Modifier.size(42.dp).align(Alignment.Center),
                            tint = Color.White,
                        )
                    }
                    LinearProgressIndicator(
                        progress = {
                            if (point.durationSeconds > 0)
                                (point.positionSeconds / point.durationSeconds)
                                    .toFloat()
                                    .coerceIn(0f, 1f)
                            else 0f
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Column(Modifier.padding(12.dp)) {
                        Text(
                            point.anime.title,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            fontWeight = FontWeight.Bold,
                        )
                        Text(
                            "Episode ${point.episode.number} • ${formatTime(point.positionSeconds)}",
                            color = Muted,
                            fontSize = 12.sp,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SearchScreen(vm: CatalogViewModel) {
    var provider by rememberSaveable { mutableStateOf("All") }
    var audio by rememberSaveable { mutableStateOf("All audio") }
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(
            vm.query,
            { vm.setQuery(it) },
            placeholder = { Text("Titles, worlds, your next favourite…") },
            leadingIcon = { Icon(Icons.Default.Search, null) },
            trailingIcon = {
                if (vm.searching)
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                else if (vm.query.isNotEmpty())
                    IconButton(onClick = { vm.setQuery("") }) {
                        Icon(Icons.Default.Close, "Clear search")
                    }
            },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { vm.setQuery(vm.query, true) }),
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp).testTag("searchField"),
        )
        LazyRow(
            Modifier.padding(vertical = 12.dp),
            contentPadding = PaddingValues(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(listOf("All") + ProviderId.values().map { it.label() }) { label ->
                FilterChip(provider == label, { provider = label }, { Text(label) })
            }
        }
        Row(
            Modifier.padding(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            listOf("All audio", "ITA dub", "Subtitled").forEach { label ->
                FilterChip(audio == label, { audio = label }, { Text(label, fontSize = 12.sp) })
            }
        }
        if (vm.query.isBlank()) {
            LazyColumn(
                contentPadding = PaddingValues(20.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                item {
                    Text(
                        "Find something worth staying up for.",
                        fontSize = 26.sp,
                        fontWeight = FontWeight.Bold,
                    )
                }
                if (vm.library.recent.isNotEmpty())
                    item {
                        Text(
                            "RECENT SEARCHES",
                            color = Muted,
                            fontSize = 11.sp,
                            letterSpacing = 2.sp,
                        )
                    }
                items(vm.library.recent) { query ->
                    Row(
                        Modifier.fillMaxWidth()
                            .clickable { vm.setQuery(query, true) }
                            .padding(vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Icons.Default.History, null, tint = Muted)
                        Text(query, Modifier.padding(start = 14.dp))
                    }
                }
            }
        } else {
            vm.searchError?.let {
                ErrorBlock("Search unavailable", it) { vm.setQuery(vm.query, true) }
            }
            val filtered =
                vm.results.filter {
                    (provider == "All" || it.provider.label() == provider) &&
                        (audio == "All audio" || if (audio == "ITA dub") it.dub else !it.dub)
                }
            if (!vm.searching && vm.searchError == null && filtered.isEmpty())
                EmptyBlock("No matches yet", "Try a different title or relax the filters.")
            LazyVerticalGrid(
                GridCells.Adaptive(105.dp),
                contentPadding = PaddingValues(20.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                items(filtered, key = { it.key() }) { anime ->
                    PosterCard(anime) { vm.open(anime) }
                }
            }
        }
    }
}

@Composable
private fun LibraryScreen(vm: CatalogViewModel) {
    LazyColumn(
        contentPadding = PaddingValues(vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(24.dp),
    ) {
        item {
            SectionHeading(
                "Your corner of the catalogue",
                "Watchlist, progress, and every world you’ve visited.",
            )
        }
        if (vm.library.resume.isNotEmpty()) item { ContinueRow(vm) }
        if (vm.library.watchlist.isNotEmpty())
            item {
                PosterRail(
                    "My List",
                    "Tap a title to play or manage episodes",
                    vm.library.watchlist,
                    vm::open,
                )
            }
        if (vm.library.history.isNotEmpty())
            item {
                PosterRail("Recently watched", "Your viewing history", vm.library.history, vm::open)
            }
        if (vm.library.watchlist.isEmpty() && vm.library.history.isEmpty())
            item {
                EmptyBlock(
                    "Make it yours",
                    "Add a title to My List or start an episode. It’ll be here when you come back.",
                )
            }
    }
}

@Composable
private fun DetailScreen(vm: CatalogViewModel, detail: DetailState) {
    val anime = detail.anime
    var download by remember { mutableStateOf<Episode?>(null) }
    val saved = vm.offline.downloads.collectAsStateWithLifecycle().value
    LazyColumn(contentPadding = PaddingValues(bottom = 32.dp)) {
        item {
            Box(Modifier.fillMaxWidth().height(310.dp)) {
                Poster(anime, Modifier.fillMaxSize())
                Box(
                    Modifier.fillMaxSize()
                        .background(
                            Brush.verticalGradient(
                                listOf(Ink.copy(alpha = .2f), Ink.copy(alpha = .35f), Ink)
                            )
                        )
                )
                IconButton(
                    onClick = vm::closeDetail,
                    modifier =
                        Modifier.align(Alignment.TopStart)
                            .padding(12.dp)
                            .background(Ink.copy(alpha = .75f), RoundedCornerShape(50)),
                ) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back")
                }
                Column(Modifier.align(Alignment.BottomStart).padding(20.dp)) {
                    Text(
                        anime.provider.label().uppercase(),
                        color = Accent,
                        fontSize = 11.sp,
                        letterSpacing = 2.sp,
                        fontWeight = FontWeight.Bold,
                    )
                    Text(
                        anime.title,
                        fontWeight = FontWeight.Black,
                        fontSize = 29.sp,
                        lineHeight = 32.sp,
                    )
                    Text(
                        listOf(
                                anime.year,
                                anime.kind,
                                anime.score.takeIf { it.isNotBlank() }?.let { "★ $it" }.orEmpty(),
                                anime.audioLabel(),
                            )
                            .filter { it.isNotBlank() }
                            .joinToString("  •  "),
                        color = Muted,
                        fontSize = 12.sp,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
            }
        }
        item {
            Column(
                Modifier.padding(horizontal = 20.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                val resume = vm.library.resume.firstOrNull { it.anime.key() == anime.key() }
                val next =
                    detail.episodes.firstOrNull { it.number !in detail.watched }
                        ?: detail.episodes.firstOrNull()
                Button(
                    onClick = { (resume?.episode ?: next)?.let { vm.play(anime, it) } },
                    enabled = next != null || resume != null,
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(10.dp),
                    contentPadding = PaddingValues(14.dp),
                ) {
                    Icon(Icons.Default.PlayArrow, null)
                    Text(
                        resume?.let { "Resume episode ${it.episode.number}" }
                            ?: "Play${next?.let { " episode ${it.number}" }.orEmpty()}",
                        fontWeight = FontWeight.Bold,
                    )
                }
                OutlinedButton(
                    onClick = { vm.toggleWatchlist(anime) },
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(10.dp),
                ) {
                    val added = vm.library.watchlist.any { it.key() == anime.key() }
                    Icon(if (added) Icons.Default.Check else Icons.Default.Add, null)
                    Text(
                        if (added) "In My List" else "Add to My List",
                        Modifier.padding(start = 8.dp),
                    )
                }
                if (anime.synopsis.isNotBlank())
                    Text(anime.synopsis, color = Color(0xFFCFD1DA), lineHeight = 22.sp)
                if (anime.genres.isNotEmpty())
                    Text(anime.genres.joinToString("  ·  "), color = Muted, fontSize = 12.sp)
                HorizontalDivider(color = Panel)
                Text("Episodes", fontSize = 21.sp, fontWeight = FontWeight.Bold)
                Text(
                    "Choose your audio before playback. Long-press is never required.",
                    color = Muted,
                    fontSize = 12.sp,
                )
            }
        }
        if (detail.loading) item { LoadingBlock("Loading episodes…") }
        detail.error?.let {
            item { ErrorBlock("Couldn’t refresh episodes", it) { vm.open(anime) } }
        }
        items(detail.episodes, key = { it.id }) { episode ->
            val watched = episode.number in detail.watched
            val cached = saved.firstOrNull { it.download.request.id == episode.key(anime) }
            Column(Modifier.padding(horizontal = 12.dp)) {
                Row(
                    Modifier.fillMaxWidth().clickable { vm.play(anime, episode) }.padding(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier.size(52.dp, 42.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .background(Panel),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            episode.number,
                            fontWeight = FontWeight.Bold,
                            color = if (watched) Muted else Color.White,
                        )
                    }
                    Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                        Text(
                            episode.title.ifBlank { "Episode ${episode.number}" },
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            fontWeight = FontWeight.Medium,
                        )
                        Text(
                            if (watched) "Watched"
                            else if (cached?.download?.state == Download.STATE_COMPLETED)
                                "Available offline"
                            else "${anime.provider.label()} • ${anime.audioLabel()}",
                            color = Muted,
                            fontSize = 11.sp,
                        )
                    }
                    IconButton(onClick = { vm.markWatched(anime, episode, !watched) }) {
                        Icon(
                            if (watched) Icons.Default.CheckCircle
                            else Icons.Default.RadioButtonUnchecked,
                            if (watched) "Mark episode ${episode.number} unwatched"
                            else "Mark episode ${episode.number} watched",
                            tint = if (watched) Accent else Muted,
                        )
                    }
                    IconButton(
                        onClick = {
                            if (cached == null) download = episode
                            else vm.destination = Destination.Downloads.also { vm.closeDetail() }
                        },
                        enabled = !vm.preparingDownload,
                    ) {
                        Icon(
                            if (cached?.download?.state == Download.STATE_COMPLETED)
                                Icons.Default.DownloadDone
                            else Icons.Default.Download,
                            "Download episode ${episode.number}",
                            tint = Muted,
                        )
                    }
                }
                HorizontalDivider(color = Panel, modifier = Modifier.padding(start = 72.dp))
            }
        }
    }
    download?.let { episode -> DownloadDialog(vm, anime, episode) { download = null } }
}

@Composable
private fun DownloadDialog(
    vm: CatalogViewModel,
    anime: Anime,
    episode: Episode,
    dismiss: () -> Unit,
) {
    var language by remember { mutableStateOf(vm.audioLanguage) }
    var height by remember { mutableStateOf(720) }
    val notification =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    AlertDialog(
        onDismissRequest = dismiss,
        title = { Text("Save episode ${episode.number}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    "Downloads stay in this app. Choose an audio preference; the source default is used if unavailable.",
                    color = Muted,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    listOf("it" to "Italian", "en" to "English", "ja" to "Japanese").forEach {
                        (code, label) ->
                        FilterChip(
                            language == code,
                            { language = code },
                            { Text(label, fontSize = 11.sp) },
                        )
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(480, 720, 1080).forEach { h ->
                        FilterChip(height == h, { height = h }, { Text("${h}p") })
                    }
                }
                Text(
                    if (vm.wifiOnly) "Wi-Fi only • change in Settings" else "Mobile data allowed",
                    color = Muted,
                    fontSize = 12.sp,
                )
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    if (Build.VERSION.SDK_INT >= 33)
                        notification.launch(Manifest.permission.POST_NOTIFICATIONS)
                    vm.download(anime, episode, language, height)
                    dismiss()
                }
            ) {
                Text("Download")
            }
        },
        dismissButton = { TextButton(onClick = dismiss) { Text("Cancel") } },
    )
}

@Composable
private fun DownloadsScreen(vm: CatalogViewModel) {
    val downloads = vm.offline.downloads.collectAsStateWithLifecycle().value
    var removing by remember { mutableStateOf<SavedEpisode?>(null) }
    LazyColumn(
        contentPadding = PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            Text(
                "Good stories. No signal needed.",
                fontSize = 27.sp,
                lineHeight = 31.sp,
                fontWeight = FontWeight.Bold,
            )
        }
        item {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedButton(onClick = { vm.offline.pause() }) { Text("Pause all") }
                OutlinedButton(
                    onClick = {
                        try {
                            vm.offline.resume()
                        } catch (e: Exception) {
                            vm.message = e.message
                        }
                    }
                ) {
                    Text("Resume all")
                }
            }
        }
        if (downloads.isEmpty())
            item {
                EmptyBlock(
                    "Nothing saved yet",
                    "Open a title and tap the download icon beside an episode. Wi-Fi is required by default.",
                )
            }
        items(downloads, key = { it.download.request.id }) { saved ->
            val d = saved.download
            val m = saved.metadata
            Surface(shape = RoundedCornerShape(14.dp), color = Panel) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(m.anime.title, fontWeight = FontWeight.Bold)
                    Text(
                        "Episode ${m.episode.number} • ${m.language.uppercase()}",
                        color = Muted,
                        fontSize = 12.sp,
                    )
                    val status =
                        when (d.state) {
                            Download.STATE_COMPLETED -> "Ready to watch offline"
                            Download.STATE_FAILED -> "Download failed — retry to refresh the stream"
                            Download.STATE_DOWNLOADING ->
                                "Downloading ${(d.percentDownloaded.takeIf { it >= 0 } ?: 0f).toInt()}%"
                            Download.STATE_REMOVING -> "Removing…"
                            Download.STATE_STOPPED -> "Paused"
                            else -> "Queued / waiting for network"
                        }
                    Text(
                        status,
                        color =
                            if (d.state == Download.STATE_COMPLETED)
                                MaterialTheme.colorScheme.secondary
                            else Muted,
                        fontSize = 12.sp,
                    )
                    if (d.state == Download.STATE_DOWNLOADING)
                        LinearProgressIndicator(
                            progress = { (d.percentDownloaded / 100f).coerceIn(0f, 1f) },
                            modifier = Modifier.fillMaxWidth(),
                        )
                    Row {
                        if (d.state == Download.STATE_COMPLETED)
                            TextButton(onClick = { vm.play(m.anime, m.episode) }) {
                                Icon(Icons.Default.PlayArrow, null)
                                Text("Watch offline")
                            }
                        if (d.state == Download.STATE_FAILED)
                            TextButton(
                                onClick = { vm.download(m.anime, m.episode, m.language, 720) }
                            ) {
                                Text("Retry")
                            }
                        Spacer(Modifier.weight(1f))
                        TextButton(onClick = { removing = saved }) {
                            Text(if (d.state == Download.STATE_COMPLETED) "Remove" else "Cancel")
                        }
                    }
                }
            }
        }
    }
    removing?.let { saved ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove saved episode?") },
            text = {
                Text(
                    "Only episode ${saved.metadata.episode.number} of ${saved.metadata.anime.title} will be removed. Your progress and watchlist stay."
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        vm.offline.remove(saved.download.request.id)
                        removing = null
                    }
                ) {
                    Text("Remove")
                }
            },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Keep") } },
        )
    }
}

@Composable
private fun SettingsScreen(vm: CatalogViewModel) {
    val settings = vm.settings
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var token by remember { mutableStateOf("") }
    var loginDialog by remember { mutableStateOf(false) }
    var mirrors by remember { mutableStateOf(false) }
    LazyColumn(
        contentPadding = PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        item {
            Text("Your experience, your rules.", fontSize = 27.sp, fontWeight = FontWeight.Bold)
        }
        item {
            PreferenceSwitch(
                "Private viewing",
                "Don’t record progress/searches or contact AniList.",
                settings?.privateMode == true,
                settings != null,
            ) {
                settings?.let { s -> vm.saveSettings(s.copy(privateMode = it)) }
            }
        }
        item {
            PreferenceSwitch(
                "Wi-Fi downloads only",
                "Keep your mobile data for something else.",
                vm.wifiOnly,
            ) {
                vm.preferences(wifi = it)
            }
        }
        item {
            PreferenceSwitch(
                "Autoplay next episode",
                "Keep the story going when an episode ends.",
                vm.autoNext,
            ) {
                vm.preferences(next = it)
            }
        }
        item {
            Text("Preferred audio", fontWeight = FontWeight.Bold)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("it" to "Italian", "en" to "English", "ja" to "Japanese").forEach {
                    (code, label) ->
                    FilterChip(
                        vm.audioLanguage == code,
                        { vm.preferences(audio = code) },
                        { Text(label) },
                    )
                }
            }
            Text(
                "You can choose any available track in the player.",
                color = Muted,
                fontSize = 12.sp,
            )
        }
        item {
            HorizontalDivider(color = Panel)
            Text(
                "AniList",
                fontSize = 22.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(top = 18.dp),
            )
        }
        item {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    if (vm.user.isBlank())
                        "Connect your profile to sync My List and watched episodes."
                    else "Connected as ${vm.user}",
                    color = Muted,
                )
                if (vm.user.isBlank()) {
                    var clientId by
                        remember(settings?.anilistClientId) {
                            mutableStateOf(
                                settings?.anilistClientId?.takeIf { it > 0u }?.toString().orEmpty()
                            )
                        }
                    OutlinedTextField(
                        clientId,
                        { clientId = it.filter(Char::isDigit).take(10) },
                        label = { Text("Your AniList OAuth client ID") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    TextButton(
                        onClick = {
                            context.startActivity(
                                Intent(
                                    Intent.ACTION_VIEW,
                                    Uri.parse("https://anilist.co/settings/developer"),
                                )
                            )
                        }
                    ) {
                        Text("Create an AniList client")
                    }
                    Button(
                        onClick = {
                            settings?.let {
                                vm.saveSettings(
                                    it.copy(anilistClientId = clientId.toUIntOrNull() ?: 0u)
                                )
                            }
                        },
                        enabled = settings != null && clientId.toUIntOrNull() != null,
                    ) {
                        Text("Save client ID")
                    }
                    OutlinedButton(
                        onClick = {
                            scope.launch {
                                try {
                                    context.startActivity(
                                        Intent(Intent.ACTION_VIEW, Uri.parse(vm.authorizeUrl()))
                                    )
                                } catch (e: Exception) {
                                    vm.message = e.message
                                }
                            }
                        },
                        enabled =
                            (settings?.anilistClientId ?: 0u) > 0u && settings?.privateMode == false,
                    ) {
                        Text("Authorize in browser")
                    }
                    OutlinedButton(
                        onClick = { loginDialog = true },
                        enabled = settings?.privateMode == false && !vm.accountBusy,
                    ) {
                        Text("Paste token / redirect URL")
                    }
                } else
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Button(
                            onClick = { vm.sync() },
                            enabled = !vm.accountBusy && settings?.privateMode == false,
                        ) {
                            Text("Sync library")
                        }
                        OutlinedButton(onClick = { vm.logout() }, enabled = !vm.accountBusy) {
                            Text("Disconnect")
                        }
                    }
                if (vm.accountBusy) LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        }
        item {
            HorizontalDivider(color = Panel)
            Text(
                "Providers & support",
                fontSize = 22.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(top = 18.dp),
            )
        }
        item {
            OutlinedButton(onClick = { mirrors = true }, enabled = settings != null) {
                Text("Provider mirrors")
            }
        }
        item {
            OutlinedButton(onClick = { vm.checkProviders() }) {
                Text("Check provider connectivity")
            }
            vm.diagnostics.forEach {
                Text(
                    it,
                    color = Muted,
                    fontSize = 12.sp,
                    modifier = Modifier.padding(vertical = 6.dp),
                )
            }
        }
        item {
            Text(
                "anime-levisione • GPL-3.0\nYour library and downloads are stored privately on this device. Removing the app removes them.",
                color = Muted,
                fontSize = 12.sp,
            )
        }
    }
    if (loginDialog)
        AlertDialog(
            onDismissRequest = {
                loginDialog = false
                token = ""
            },
            title = { Text("Connect AniList") },
            text = {
                Column {
                    Text(
                        "Paste the token or full redirect URL from your authorization page. It stays in app-private storage.",
                        color = Muted,
                    )
                    OutlinedTextField(
                        token,
                        { token = it.take(16384) },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        label = { Text("Token or redirect URL") },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        vm.login(token)
                        token = ""
                        loginDialog = false
                    },
                    enabled = token.isNotBlank(),
                ) {
                    Text("Connect")
                }
            },
            dismissButton = {
                TextButton(
                    onClick = {
                        token = ""
                        loginDialog = false
                    }
                ) {
                    Text("Cancel")
                }
            },
        )
    if (mirrors && settings != null) {
        var world by remember { mutableStateOf(settings.animeworldUrl) }
        var unity by remember { mutableStateOf(settings.animeunityUrl) }
        var community by remember { mutableStateOf(settings.streamingcommunityUrl) }
        AlertDialog(
            onDismissRequest = { mirrors = false },
            title = { Text("Provider mirrors") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("HTTPS URLs only. Domain healing remains automatic.", color = Muted)
                    OutlinedTextField(
                        world,
                        { world = it },
                        label = { Text("AnimeWorld") },
                        singleLine = true,
                    )
                    OutlinedTextField(
                        unity,
                        { unity = it },
                        label = { Text("AnimeUnity") },
                        singleLine = true,
                    )
                    OutlinedTextField(
                        community,
                        { community = it },
                        label = { Text("StreamingCommunity") },
                        singleLine = true,
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        vm.saveSettings(
                            settings.copy(
                                animeworldUrl = world.trim(),
                                animeunityUrl = unity.trim(),
                                streamingcommunityUrl = community.trim(),
                            )
                        )
                        mirrors = false
                    }
                ) {
                    Text("Save")
                }
            },
            dismissButton = { TextButton(onClick = { mirrors = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun PreferenceSwitch(
    title: String,
    hint: String,
    checked: Boolean,
    enabled: Boolean = true,
    change: (Boolean) -> Unit,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 14.dp)) {
            Text(title, fontWeight = FontWeight.Bold)
            Text(hint, color = Muted, fontSize = 12.sp, lineHeight = 17.sp)
        }
        Switch(checked, change, enabled = enabled)
    }
}

@Composable
fun LoadingBlock(label: String) {
    Row(
        Modifier.fillMaxWidth().padding(24.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
        Text(label, color = Muted)
    }
}

@Composable
fun EmptyBlock(title: String, hint: String) {
    Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Icon(Icons.Default.Movie, null, Modifier.size(40.dp), tint = Accent)
        Text(title, fontWeight = FontWeight.Bold, fontSize = 23.sp)
        Text(hint, color = Muted, lineHeight = 22.sp)
    }
}

@Composable
fun ErrorBlock(title: String, hint: String, retry: () -> Unit) {
    Surface(color = Panel, shape = RoundedCornerShape(12.dp), modifier = Modifier.padding(16.dp)) {
        Column(Modifier.padding(16.dp)) {
            Text(title, fontWeight = FontWeight.Bold)
            Text(
                hint.take(350),
                color = Muted,
                fontSize = 12.sp,
                modifier = Modifier.padding(vertical = 8.dp),
            )
            TextButton(onClick = retry) { Text("Try again") }
        }
    }
}

fun formatTime(seconds: Double): String {
    val total = seconds.toLong().coerceAtLeast(0)
    return "%d:%02d".format(total / 60, total % 60)
}
