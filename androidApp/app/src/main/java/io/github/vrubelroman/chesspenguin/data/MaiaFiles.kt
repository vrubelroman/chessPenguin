package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import io.github.vrubelroman.chesspenguin.R
import kotlinx.coroutines.CoroutineScope

/// Maia's weights (android-play.md §31.2): the small model (`maia3-5m.pt`, 21 MB) of a
/// pinned revision, downloaded on request from the authors' Hugging Face page — the
/// file is theirs, not shipped in the APK. The same file as the desktop app's.
class MaiaFiles(context: Context, scope: CoroutineScope) {
    val download = CheckedDownload(
        context,
        scope,
        FileSpec(
            dir = "maia",
            name = "maia3-5m.pt",
            url = "https://huggingface.co/UofTCSSLab/Maia3-5M/resolve/b6559de2398d7140b985f28fd2c19fb5e47ddabe/maia3-5m.pt",
            size = 20_968_049,
            sha256 = "ba14208b2992d85502f5fb501934abf6aaaeb355e9f3fdf90e326911f562524f",
            title = context.getString(R.string.maia_download_title),
            prefs = "maia",
        ),
    )
}
