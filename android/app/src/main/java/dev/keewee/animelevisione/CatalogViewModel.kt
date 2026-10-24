@file:androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)

package dev.keewee.animelevisione

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadRequest
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.anime_levisione_core.*

enum class Destination {
    Home,
    Search,
    Library,
    Downloads,
    Settings,
}

data class LibrarySnapshot(
    val watchlist: List<Anime> = emptyList(),
    val resume: List<ResumePoint> = emptyList(),
    val history: List<Anime> = emptyList(),
    val recent: List<String> = emptyList(),
)

data class DetailState(
    val anime: Anime,
    val episodes: List<Episode> = emptyList(),
    val watched: Set<String> = emptySet(),
    val loading: Boolean = true,
    val error: String? = null,
)

data class Playback(
    val anime: Anime,
    val episode: Episode,
    val episodes: List<Episode>,
    val stream: StreamInfo,
    val resume: Double,
    val wasCompleted: Boolean,
    val download: DownloadRequest? = null,
    val session: Long = System.nanoTime(),
)

class CatalogViewModel(application: Application) : AndroidViewModel(application) {
    private val app = application as AnimeApplication
    val repository = app.repository
    val offline = app.offline
    var destination by mutableStateOf(Destination.Home)
    var catalogue by mutableStateOf<List<Anime>>(emptyList())
        private set

    var catalogueLoading by mutableStateOf(true)
        private set

    var catalogueError by mutableStateOf<String?>(null)
        private set

    var query by mutableStateOf("")
        private set

    var results by mutableStateOf<List<Anime>>(emptyList())
        private set

    var searching by mutableStateOf(false)
        private set

    var searchError by mutableStateOf<String?>(null)
        private set

    var detail by mutableStateOf<DetailState?>(null)
        private set

    var playback by mutableStateOf<Playback?>(null)
        private set

    var resolving by mutableStateOf(false)
        private set

    var library by mutableStateOf(LibrarySnapshot())
        private set

    var settings by mutableStateOf<AppSettings?>(null)
        private set

    var user by mutableStateOf("")
        private set

    var message by mutableStateOf<String?>(null)
    var accountBusy by mutableStateOf(false)
        private set

    var diagnostics by mutableStateOf<List<String>>(emptyList())
        private set

    var audioLanguage by mutableStateOf(repository.preferences.getString("audio", "it") ?: "it")
        private set

    var wifiOnly by mutableStateOf(repository.preferences.getBoolean("wifi", true))
        private set

    var autoNext by mutableStateOf(repository.preferences.getBoolean("autoNext", false))
        private set

    var preparingDownload by mutableStateOf(false)
        private set

    private var searchGeneration = 0
    private var searchJob: Job? = null
    private var detailJob: Job? = null
    private var playJob: Job? = null
    private var detailGeneration = 0
    private var libraryGeneration = 0
    private val progressWrites = Mutex()

    init {
        offline.wifiOnly(wifiOnly)
        viewModelScope.launch {
            attempt {
                settings = repository.call { it.settings() }
                user = repository.call { it.anilistUser() }
                refreshLibrary()
            }
        }
        loadCatalogue()
        viewModelScope.launch {
            while (true) {
                delay(2000)
                offline.refresh()
            }
        }
    }

    private suspend fun attempt(block: suspend () -> Unit) {
        try {
            block()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            message = e.message ?: "Something went wrong. Please retry."
        }
    }

    suspend fun refreshLibrary() {
        val generation = ++libraryGeneration
        val snapshot =
            repository.call {
                LibrarySnapshot(
                    it.watchlist(),
                    it.continueWatching(),
                    it.history(),
                    it.recentSearches(),
                )
            }
        if (generation == libraryGeneration) library = snapshot
    }

    fun loadCatalogue() {
        if (!catalogueLoading) catalogueLoading = true
        viewModelScope.launch {
            catalogueError = null
            try {
                catalogue = repository.call { it.search("") }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                catalogueError = e.message
            } finally {
                catalogueLoading = false
            }
        }
    }

    fun setQuery(value: String, immediate: Boolean = false) {
        query = value
        val generation = ++searchGeneration
        searchJob?.cancel()
        if (value.isBlank()) {
            results = emptyList()
            searching = false
            searchError = null
            return
        }
        searchJob =
            viewModelScope.launch {
                searching = true
                searchError = null
                try {
                    if (!immediate) delay(350)
                    results = repository.call { it.search(value) }
                    refreshLibrary()
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    searchError = e.message
                } finally {
                    if (generation == searchGeneration) searching = false
                }
            }
    }

    fun open(anime: Anime) {
        detailJob?.cancel()
        val generation = ++detailGeneration
        val cachedEpisodes =
            offline.downloads.value
                .filter { it.metadata.anime.key() == anime.key() }
                .map { it.metadata.episode }
                .distinctBy { it.id }
        detail = DetailState(anime, cachedEpisodes)
        detailJob =
            viewModelScope.launch {
                try {
                    val watched = repository.call { it.watchedEpisodes(anime).toSet() }
                    if (generation != detailGeneration) return@launch
                    detail = detail?.copy(watched = watched)
                    val loaded = repository.call { it.details(anime) }
                    if (generation != detailGeneration) return@launch
                    detail = DetailState(loaded.anime, loaded.episodes, watched, false)
                    if (settings?.privateMode != true) {
                        // AniList failures must not remove otherwise playable provider results.
                        val enriched =
                            try {
                                repository.call { it.metadata(loaded.anime) }
                            } catch (e: CancellationException) {
                                throw e
                            } catch (_: Exception) {
                                null
                            }
                        if (generation == detailGeneration && enriched != null)
                            detail = detail?.copy(anime = enriched)
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    if (generation == detailGeneration)
                        detail = detail?.copy(loading = false, error = e.message)
                }
            }
    }

    fun closeDetail() {
        ++detailGeneration
        detailJob?.cancel()
        cancelResolve()
        detail = null
    }

    fun toggleWatchlist(anime: Anime) =
        viewModelScope.launch {
            attempt {
                val added = repository.call { it.toggleWatchlist(anime) }
                refreshLibrary()
                message = if (added) "Added to My List" else "Removed from My List"
                if (user.isNotEmpty() && settings?.privateMode == false)
                    repository.call { it.syncWatchlist(anime, added) }
            }
        }

    fun markWatched(anime: Anime, episode: Episode, watched: Boolean) =
        viewModelScope.launch {
            attempt {
                repository.call { it.markWatched(anime, episode, watched) }
                refreshLibrary()
                if (detail?.anime?.key() == anime.key())
                    detail =
                        detail?.copy(
                            watched = repository.call { it.watchedEpisodes(anime).toSet() }
                        )
                if (watched && user.isNotEmpty() && settings?.privateMode == false)
                    repository.call { it.syncWatched(anime, episode) }
            }
        }

    fun play(
        anime: Anime,
        episode: Episode,
        episodes: List<Episode> = detail?.episodes.orEmpty(),
        restart: Boolean = false,
    ) {
        playJob?.cancel()
        resolving = true
        playJob =
            viewModelScope.launch {
                try {
                    val saved =
                        offline.downloads.value.firstOrNull {
                            it.download.state == Download.STATE_COMPLETED &&
                                it.download.request.id == episode.key(anime)
                        }
                    val stream =
                        saved?.let {
                            StreamInfo(it.download.request.uri.toString(), it.metadata.referer)
                        } ?: repository.call { it.stream(anime, episode) }
                    val progress = repository.call { it.progress(anime, episode) }
                    val sequence =
                        if (episodes.isNotEmpty()) episodes
                        else if (saved != null) {
                            offline.downloads.value
                                .filter { it.metadata.anime.key() == anime.key() }
                                .map { it.metadata.episode }
                                .distinctBy { it.id }
                                .sortedBy { it.number.toDoubleOrNull() ?: Double.MAX_VALUE }
                        } else
                            try {
                                repository.call { it.episodes(anime) }
                            } catch (e: CancellationException) {
                                throw e
                            } catch (_: Exception) {
                                listOf(episode)
                            }
                    playback =
                        Playback(
                            anime,
                            episode,
                            sequence,
                            stream,
                            if (restart || progress?.completed == true) 0.0
                            else progress?.positionSeconds ?: 0.0,
                            progress?.completed ?: false,
                            saved?.download?.request,
                        )
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    message = e.message
                } finally {
                    resolving = false
                }
            }
    }

    fun cancelResolve() {
        playJob?.cancel()
        resolving = false
    }

    fun stopPlayback() {
        cancelResolve()
        playback = null
        viewModelScope.launch { attempt { refreshLibrary() } }
    }

    fun persist(
        playback: Playback,
        seconds: Double,
        duration: Double,
        completed: Boolean,
        sync: Boolean,
    ) {
        // Also retain the live position for Activity recreation, including private sessions.
        if (this.playback?.session == playback.session)
            this.playback = this.playback?.copy(resume = seconds, wasCompleted = completed)
        if (settings?.privateMode == true) return
        viewModelScope.launch {
            attempt {
                progressWrites.withLock {
                    repository.call {
                        it.recordProgress(
                            playback.anime,
                            playback.episode,
                            seconds,
                            duration,
                            completed,
                        )
                    }
                }
                refreshLibrary()
                if (sync && user.isNotEmpty() && settings?.privateMode == false)
                    repository.call { it.syncWatched(playback.anime, playback.episode) }
            }
        }
    }

    fun saveSettings(value: AppSettings) =
        viewModelScope.launch {
            attempt {
                repository.call { it.saveSettings(value) }
                settings = value
                message = "Settings saved"
            }
        }

    fun preferences(
        audio: String = audioLanguage,
        wifi: Boolean = wifiOnly,
        next: Boolean = autoNext,
    ) {
        val language =
            when (audio.substringBefore('-')) {
                "ita" -> "it"
                "eng" -> "en"
                "jpn" -> "ja"
                else -> audio
            }
        audioLanguage = language
        wifiOnly = wifi
        autoNext = next
        repository.preferences
            .edit()
            .putString("audio", language)
            .putBoolean("wifi", wifi)
            .putBoolean("autoNext", next)
            .apply()
        offline.wifiOnly(wifi)
    }

    fun checkProviders() =
        viewModelScope.launch {
            attempt {
                diagnostics = listOf("Checking providers…")
                diagnostics = repository.call { it.diagnostics() }
            }
        }

    fun login(token: String) =
        viewModelScope.launch {
            accountBusy = true
            try {
                attempt {
                    user = repository.call { it.anilistLogin(token) }
                    message = "Connected to AniList as $user"
                }
            } finally {
                accountBusy = false
            }
        }

    fun logout() =
        viewModelScope.launch {
            attempt {
                repository.call { it.anilistLogout() }
                user = ""
                message = "Disconnected from AniList"
            }
        }

    fun sync() =
        viewModelScope.launch {
            accountBusy = true
            try {
                attempt {
                    val report = repository.call { it.anilistSync() }
                    message = report.joinToString("\n").ifBlank { "Nothing to sync yet" }
                }
            } finally {
                accountBusy = false
            }
        }

    suspend fun authorizeUrl() = repository.call { it.anilistAuthorizeUrl() }

    fun download(anime: Anime, episode: Episode, language: String, height: Int) =
        viewModelScope.launch {
            if (preparingDownload) return@launch
            preparingDownload = true
            try {
                attempt {
                    val stream = repository.call { it.stream(anime, episode) }
                    val metadata = DownloadMetadata(anime, episode, stream.referer, language)
                    offline.add(offline.prepare(metadata, stream, height))
                    message = "Episode queued for download"
                }
            } finally {
                preparingDownload = false
            }
        }
}
