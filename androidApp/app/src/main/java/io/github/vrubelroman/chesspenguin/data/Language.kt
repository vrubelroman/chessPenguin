package io.github.vrubelroman.chesspenguin.data

import android.app.LocaleManager
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.LocaleList

/// The interface languages; "" follows the system (android-ui.md §9).
val LANGUAGES = listOf("", "en", "ru")

/// The language chosen in the app. Android 13+ keeps it itself (and shows it in the
/// system settings of the app); on Android 12 the app keeps it and applies it to its
/// activity.
object Language {
    private const val KEY = "language"

    fun get(context: Context): String = if (Build.VERSION.SDK_INT >= 33) {
        context.getSystemService(LocaleManager::class.java).applicationLocales.get(0)?.language ?: ""
    } else {
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).getString(KEY, "") ?: ""
    }

    /// Returns true when the activity has to be recreated to show it (Android 12).
    fun set(context: Context, tag: String): Boolean {
        if (Build.VERSION.SDK_INT >= 33) {
            context.getSystemService(LocaleManager::class.java).applicationLocales =
                if (tag.isEmpty()) LocaleList.getEmptyLocaleList() else LocaleList.forLanguageTags(tag)
            return false
        }
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putString(KEY, tag).apply()
        return true
    }

    /// The activity's context in the chosen language (Android 12 only).
    fun wrap(base: Context): Context {
        if (Build.VERSION.SDK_INT >= 33) return base
        val tag = base.getSharedPreferences("settings", Context.MODE_PRIVATE).getString(KEY, "") ?: ""
        if (tag.isEmpty()) return base
        val config = Configuration(base.resources.configuration)
        config.setLocales(LocaleList.forLanguageTags(tag))
        return base.createConfigurationContext(config)
    }
}
