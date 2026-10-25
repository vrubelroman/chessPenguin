package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import android.media.AudioAttributes
import android.media.SoundPool

/// What a sound says (android-play.md §33).
enum class Sound(val file: String) {
    Move("Move"),
    Capture("Capture"),
    Check("Check"),
    Defeat("Defeat"),
    Success("Success"),
    Wrong("Wrong"),
}

/// The desktop's «SFX» set, loaded once; played when the setting «Sound» is on.
class Sounds(context: Context, private val settings: Settings) {
    private val pool = SoundPool.Builder()
        .setMaxStreams(2)
        .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_GAME).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
        .build()
    private val ids: Map<Sound, Int> = Sound.entries.associateWith { s ->
        runCatching { context.assets.openFd("sounds/${s.file}.ogg").use { pool.load(it, 1) } }.getOrDefault(0)
    }

    fun play(sound: Sound) {
        if (!settings.prefs.value.sound) return
        val id = ids[sound] ?: return
        if (id != 0) pool.play(id, 1f, 1f, 1, 0, 1f)
    }

    /// The sound of a move by its SAN: a check, a capture, or a move.
    fun move(san: String) = play(
        when {
            san.endsWith("+") || san.endsWith("#") -> Sound.Check
            'x' in san -> Sound.Capture
            else -> Sound.Move
        },
    )
}
