package dev.keewee.animelevisione

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

val Accent = Color(0xFFFF395D)
val Ink = Color(0xFF080A10)
val Panel = Color(0xFF141720)
val Muted = Color(0xFFA8ADBC)

@Composable
fun LevisioneTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme =
            darkColorScheme(
                primary = Accent,
                onPrimary = Color.White,
                background = Ink,
                onBackground = Color(0xFFF4F4F8),
                surface = Panel,
                onSurface = Color(0xFFF4F4F8),
                surfaceVariant = Color(0xFF242833),
                onSurfaceVariant = Muted,
                secondary = Color(0xFFC6BDFF),
                error = Color(0xFFFF9DAA),
            ),
        content = content,
    )
}
