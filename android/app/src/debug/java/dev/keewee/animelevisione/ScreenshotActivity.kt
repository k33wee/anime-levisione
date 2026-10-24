package dev.keewee.animelevisione

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.core.view.WindowCompat

/** Empty instrumentation host: unlike MainActivity, it starts no catalogue requests. */
class ScreenshotActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowCompat.getInsetsController(window, window.decorView).isAppearanceLightStatusBars =
            false
        WindowCompat.getInsetsController(window, window.decorView).isAppearanceLightNavigationBars =
            false
    }
}
