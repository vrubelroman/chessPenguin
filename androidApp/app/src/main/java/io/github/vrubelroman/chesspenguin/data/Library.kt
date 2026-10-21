package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import uniffi.mobile_api.ApiException
import uniffi.mobile_api.Database
import uniffi.mobile_api.openBytes
import uniffi.mobile_api.openFile
import java.io.File
import java.util.UUID

/// Where a database comes from.
@Serializable
enum class Source {
    /// One of the sample games of the desktop app, inside the app (assets/examples).
    Sample,

    /// A file the user picked; read in place through its persisted permission.
    File,
}

/// A database on the list. The file itself is never changed.
@Serializable
data class DbEntry(
    val id: String,
    val name: String,
    val source: Source,
    /// The asset path of a sample, the content URI of a file.
    val location: String,
    /// The file's name, for its extension (PGN, .zip, .7z, .cbv).
    val fileName: String,
    val games: Int? = null,
    val format: String? = null,
    /// A sample shows the name of the current language until it is renamed (displayName).
    val renamed: Boolean = false,
)

/// The databases on the list and the open ones (docs/design/android-ui.md §3).
class Library(private val context: Context) {
    private val file = File(context.filesDir, "library.json")
    private val json = Json { ignoreUnknownKeys = true }
    private val _entries = MutableStateFlow(load())
    val entries: StateFlow<List<DbEntry>> = _entries.asStateFlow()

    /// Open databases by entry id (they are small: phone-sized).
    private val open = HashMap<String, Database>()
    private val lock = Mutex()

    private fun load(): List<DbEntry> {
        if (!file.exists()) return samples().also { save(it) }
        return runCatching { json.decodeFromString<List<DbEntry>>(file.readText()) }.getOrDefault(emptyList())
    }

    private fun save(list: List<DbEntry>) {
        val tmp = File(file.path + ".tmp")
        tmp.writeText(json.encodeToString(list))
        tmp.renameTo(file)
    }

    private fun update(change: (List<DbEntry>) -> List<DbEntry>) {
        val next = change(_entries.value)
        _entries.value = next
        save(next)
    }

    /// The sample games of the desktop app: on the list from the first start.
    private fun samples(): List<DbEntry> {
        val names = listOf(
            "classics.pgn" to context.getString(io.github.vrubelroman.chesspenguin.R.string.sample_classics),
            "alekhine.pgn" to context.getString(io.github.vrubelroman.chesspenguin.R.string.sample_alekhine),
        )
        return names.map { (f, name) -> DbEntry(id = "sample-$f", name = name, source = Source.Sample, location = "examples/$f", fileName = f) }
    }

    /// Puts the sample games back on the list (those not on it), at its end.
    fun addSamples() = update { list -> list + samples().filter { s -> list.none { it.id == s.id } } }

    fun entry(id: String): DbEntry? = _entries.value.firstOrNull { it.id == id }

    /// Adds a file picked with the system picker; its read permission is kept across
    /// restarts. Returns the new entry, or the one already on the list.
    fun addFile(uri: Uri): DbEntry {
        _entries.value.firstOrNull { it.location == uri.toString() }?.let { return it }
        runCatching {
            context.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val fileName = displayName(uri) ?: "database.pgn"
        val entry = DbEntry(
            id = UUID.randomUUID().toString(),
            name = fileName.substringBeforeLast('.'),
            source = Source.File,
            location = uri.toString(),
            fileName = fileName,
        )
        update { it + entry }
        return entry
    }

    fun rename(id: String, name: String) = update { list -> list.map { if (it.id == id) it.copy(name = name, renamed = true) else it } }

    /// Takes the database off the list (the file stays where it is). Returns its place,
    /// for "Undo".
    fun remove(id: String): Pair<Int, DbEntry>? {
        val list = _entries.value
        val i = list.indexOfFirst { it.id == id }
        if (i < 0) return null
        val e = list[i]
        update { it.filterNot { x -> x.id == id } }
        open.remove(id)?.close()
        if (e.source == Source.File) {
            runCatching {
                context.contentResolver.releasePersistableUriPermission(Uri.parse(e.location), Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        }
        return i to e
    }

    fun restore(place: Int, entry: DbEntry) {
        if (entry.source == Source.File) {
            runCatching {
                context.contentResolver.takePersistableUriPermission(Uri.parse(entry.location), Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        }
        update { it.toMutableList().apply { add(place.coerceIn(0, size), entry) } }
    }

    private fun displayName(uri: Uri): String? = runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    }.getOrNull()

    /// The open database of an entry (opened in the background the first time).
    suspend fun database(id: String): Result<Database> = lock.withLock {
        open[id]?.let { return@withLock Result.success(it) }
        val e = entry(id) ?: return@withLock Result.failure(IllegalArgumentException("no database $id"))
        val result = withContext(Dispatchers.IO) {
            runCatching {
                when (e.source) {
                    Source.Sample -> openBytes(e.fileName, context.assets.open(e.location).use { it.readBytes() })
                    Source.File -> {
                        val pfd = context.contentResolver.openFileDescriptor(Uri.parse(e.location), "r")
                            ?: throw ApiException.Io(detail = "no access to the file")
                        openFile(e.fileName, pfd.detachFd())
                    }
                }
            }
        }
        result.onSuccess { db ->
            open[id] = db
            val games = db.len().toInt()
            val format = db.format()
            if (e.games != games || e.format != format) {
                update { list -> list.map { if (it.id == id) it.copy(games = games, format = format) else it } }
            }
        }
        result
    }
}
