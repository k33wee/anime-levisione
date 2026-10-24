@file:OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)

package dev.keewee.animelevisione

import android.graphics.Bitmap
import android.net.Uri
import androidx.activity.compose.setContent
import androidx.compose.runtime.MutableState
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.viewModelScope
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.json.JSONArray
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import uniffi.anime_levisione_core.*

/** Opt-in documentation capture. Only test/debug APKs contain this harness and its assets. */
class DemoScreenshotsTest {
    @get:Rule val compose = createAndroidComposeRule<ScreenshotActivity>()
    private val instrumentation
        get() = InstrumentationRegistry.getInstrumentation()

    private data class Entry(val anime: Anime, val episodes: List<Episode>)

    private fun readCatalog(): List<Entry> {
        val assets = instrumentation.context.assets
        val rows =
            JSONArray(assets.open("demo/catalog.json").bufferedReader().use { it.readText() })
        val providers =
            listOf(ProviderId.ANIME_WORLD, ProviderId.ANIME_UNITY, ProviderId.STREAMING_COMMUNITY)
        return (0 until rows.length()).map { i ->
            val row = rows.getJSONObject(i)
            val id = row.getString("id")
            val cover = File(compose.activity.cacheDir, "$id.png")
            assets.open("demo/$id.png").use { input ->
                cover.outputStream().use { input.copyTo(it) }
            }
            val genres = row.getJSONArray("genres")
            val anime =
                Anime(
                    provider = providers[i % providers.size],
                    id = id,
                    title = row.getString("title"),
                    aliases = emptyList(),
                    synopsis = row.getString("synopsis"),
                    year = row.getString("year"),
                    kind = "TV",
                    status = "Finished",
                    score = row.getString("score"),
                    episodes = row.getInt("episodes").toUInt(),
                    dub = row.getBoolean("dub"),
                    genres = (0 until genres.length()).map { genres.getString(it) },
                    pageUrl = "",
                    coverUrl = Uri.fromFile(cover).toString(),
                )
            Entry(
                anime,
                (1..row.getInt("episodes")).map { n -> Episode("$id-$n", "$n", "Episode $n") },
            )
        }
    }

    private fun offlineViewModel(): CatalogViewModel {
        lateinit var vm: CatalogViewModel
        compose.runOnUiThread {
            // Queue, then cancel the constructor's jobs before any can reach the native API.
            // Restore Main immediately so Compose/Media3 continue to use the real UI dispatcher.
            Dispatchers.setMain(StandardTestDispatcher())
            try {
                vm = CatalogViewModel(compose.activity.application)
                vm.viewModelScope.cancel()
            } finally {
                Dispatchers.resetMain()
            }
        }
        return vm
    }

    /** Seed private UI state in the test, without adding fixture hooks to production code. */
    @Suppress("UNCHECKED_CAST")
    private fun seed(vm: CatalogViewModel, name: String, value: Any?) {
        val field = CatalogViewModel::class.java.getDeclaredField("${name}\$delegate")
        field.isAccessible = true
        (field.get(vm) as MutableState<Any?>).value = value
    }

    private fun capture(name: String) {
        compose.waitForIdle()
        // Full-display capture includes the audio dialog and Media3's SurfaceView.
        val bitmap = checkNotNull(instrumentation.uiAutomation.takeScreenshot())
        val target = File(compose.activity.cacheDir, "screenshots/$name.png")
        target.parentFile?.mkdirs()
        target.outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        bitmap.recycle()
        println("SCREENSHOT-SAVED ${target.absolutePath}")
    }

    @Test
    fun captureReadmeScreenshotsFromSyntheticCatalog() {
        assumeTrue(InstrumentationRegistry.getArguments().getString("captureScreenshots") == "true")
        val catalog = readCatalog()
        val vm = offlineViewModel()
        val nativeApi =
            AppRepository::class.java.getDeclaredField("library").apply { isAccessible = true }
        assertNull("No provider API may be initialized", nativeApi.get(vm.repository))
        compose.runOnUiThread {
            seed(vm, "catalogue", catalog.map { it.anime })
            seed(vm, "catalogueLoading", false)
            seed(
                vm,
                "settings",
                AppSettings(
                    false,
                    "https://world.example",
                    "https://unity.example",
                    "https://community.example",
                    0u,
                ),
            )
            seed(
                vm,
                "library",
                LibrarySnapshot(
                    watchlist = listOf(catalog[0].anime, catalog[5].anime, catalog[6].anime),
                    resume =
                        listOf(
                            ResumePoint(
                                catalog[1].anime,
                                catalog[1].episodes[4],
                                462.0,
                                1440.0,
                                false,
                            )
                        ),
                    history = listOf(catalog[2].anime),
                    recent = listOf("orbit", "tram"),
                ),
            )
            compose.activity.setContent { LevisioneTheme { LevisioneApp(vm) } }
        }
        compose.onAllNodesWithText("Orbit Garden", substring = true).onFirst().assertIsDisplayed()
        // Wait for Coil to decode the local PNG covers (no remote image loaders).
        Thread.sleep(1800)
        capture("android-home")

        compose.runOnUiThread {
            seed(
                vm,
                "detail",
                DetailState(
                    catalog[0].anime,
                    catalog[0].episodes,
                    setOf("orbit-garden-1", "orbit-garden-2"),
                    false,
                ),
            )
        }
        compose.onNodeWithText("Play episode 1").assertIsDisplayed()
        Thread.sleep(500)
        capture("android-detail")

        val clip = File(compose.activity.cacheDir, "demo-bilingual.mp4")
        instrumentation.context.assets.open("bilingual.mp4").use { input ->
            clip.outputStream().use { input.copyTo(it) }
        }
        compose.runOnUiThread {
            seed(
                vm,
                "playback",
                Playback(
                    catalog[0].anime,
                    catalog[0].episodes[0],
                    catalog[0].episodes,
                    StreamInfo(Uri.fromFile(clip).toString(), ""),
                    0.0,
                    false,
                ),
            )
        }
        compose.waitUntil(20000) {
            compose.onAllNodesWithText("Choose audio").fetchSemanticsNodes().isNotEmpty()
        }
        compose.onAllNodesWithText("English", substring = true).onFirst().assertIsDisplayed()
        // Advance Compose's virtual clock for the real player's duration poll.
        compose.mainClock.advanceTimeBy(1200)
        // Dialogs have a separate Android window: wait for its first display frame too.
        Thread.sleep(500)
        capture("android-player-audio")
        compose.onAllNodesWithText("English", substring = true).onFirst().performClick()
        compose.waitUntil(20000) {
            compose
                .onAllNodesWithText("Playing", substring = true)
                .fetchSemanticsNodes()
                .isNotEmpty()
        }
        Thread.sleep(1200)
        compose.mainClock.advanceTimeBy(1500)
        compose.onNodeWithTag("playbackStatus").assertTextContains("0:30", substring = true)
        compose.onNodeWithTag("playPause").performClick()
        capture("android-player-controls")
        compose.onNodeWithTag("toggleFullscreen").performClick()
        compose.waitUntil(15000) {
            val bounds = compose.onRoot().fetchSemanticsNode().boundsInRoot
            bounds.width > bounds.height
        }
        Thread.sleep(500)
        capture("android-fullscreen")
        compose.onNodeWithTag("toggleFullscreen").performClick()
        compose.waitUntil(15000) {
            val bounds = compose.onRoot().fetchSemanticsNode().boundsInRoot
            bounds.height > bounds.width
        }
        compose.runOnUiThread {
            seed(vm, "playback", null)
            seed(vm, "detail", null)
            vm.destination = Destination.Settings
        }
        compose.onNodeWithText("Private viewing").assertIsDisplayed()
        capture("android-settings")
        compose.runOnUiThread { compose.activity.setContent {} }
        assertNull(
            "The captures must never initialize the provider API",
            nativeApi.get(vm.repository),
        )
        clip.delete()
    }
}
