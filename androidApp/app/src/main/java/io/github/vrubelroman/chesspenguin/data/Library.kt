package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
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
import uniffi.mobile_api.FolderFile
import uniffi.mobile_api.openBytes
import uniffi.mobile_api.openChessbase
import uniffi.mobile_api.openFile
import java.io.File
import java.util.UUID

/// Where a database comes from.
@Serializable
enum class Source {
    /// One of the sample games of the desktop app, inside the app (assets/examples).
    Sample,

    /// A file the user picked, or found in a picked folder; read in place through its
    /// persisted permission (of the file, or of the folder: [DbEntry.tree]).
    File,

    /// A ChessBase database (`.cbh` with `.cbg`, `.cba`, … beside it) in a picked folder.
    ChessBase,
}

/// A database on the list. The file itself is never changed.
@Serializable
data class DbEntry(
    val id: String,
    val name: String,
    val source: Source,
    /// The asset path of a sample, the content URI of a file; for a ChessBase database,
    /// the URI of the folder it is in.
    val location: String,
    /// The file's name, for its extension (PGN, .zip, .7z, .cbv, .cbh).
    val fileName: String,
    /// The picked folder whose permission covers this database (none for a picked file).
    val tree: String? = null,
    /// The file's size when it was added: with the name, how the same file is known when
    /// it is picked again through another way (a folder, or on its own).
    val size: Long? = null,
    val games: Int? = null,
    val format: String? = null,
    /// A sample shows the name of the current language until it is renamed (displayName).
    val renamed: Boolean = false,
)

/// A database found in a picked folder, offered to be added.
data class Found(
    val name: String,
    val fileName: String,
    /// What [DbEntry.location] becomes.
    val location: String,
    val source: Source,
    /// Where it lies inside the picked folder ("" at its top), shown with the name.
    val path: String,
    val size: Long? = null,
    /// A ChessBase database without its games file (`.cbg`): it cannot be opened.
    val incomplete: Boolean = false,
    /// Already on the list.
    val added: Boolean = false,
)

/// A file of a ChessBase database is not beside its `.cbh` any more.
class MissingPart(val file: String) : Exception(file)

/// Names as people sort them: "g2" before "g10", letters without case.
fun naturalOrder(a: String, b: String): Int {
    val chunk = Regex("\\d+|\\D+")
    val x = chunk.findAll(a.lowercase()).map { it.value }.toList()
    val y = chunk.findAll(b.lowercase()).map { it.value }.toList()
    for (i in 0 until minOf(x.size, y.size)) {
        val (p, q) = x[i] to y[i]
        val c = if (p[0].isDigit() && q[0].isDigit()) {
            compareValuesBy(p, q, { it.trimStart('0').length }, { it.trimStart('0') })
        } else {
            p.compareTo(q)
        }
        if (c != 0) return c
    }
    return x.size - y.size
}

/// What a picked folder holds.
data class FolderScan(val tree: Uri, val name: String, val found: List<Found>)

/// The databases on the list and the open ones (docs/design/android-ui.md §3).
class Library(private val context: Context) {
    private val file = File(context.filesDir, "library.json")
    private val json = Json { ignoreUnknownKeys = true }
    private val _entries = MutableStateFlow(load())
    val entries: StateFlow<List<DbEntry>> = _entries.asStateFlow()

    /// Open databases by entry id (they are small: phone-sized).
    private val open = HashMap<String, Database>()
    private val lock = Mutex()

    /// Taken off the list, "Undo" still offered: their access is kept.
    private val pending = ArrayList<DbEntry>()

    init {
        releaseUnused()
    }

    /// Gives back the access to files and folders no database on the list uses (a
    /// folder picked, then the app closed before anything was added from it; the app
    /// closed while "Undo" was offered).
    private fun releaseUnused() {
        val used = _entries.value.mapNotNull { it.tree ?: it.location.takeIf { _ -> it.source != Source.Sample } }.toSet()
        for (p in context.contentResolver.persistedUriPermissions) {
            if (p.uri.toString() !in used) {
                runCatching { context.contentResolver.releasePersistableUriPermission(p.uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
            }
        }
    }

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
        // Taken again for a file already on the list: picking it grants a new permission.
        runCatching {
            context.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val fileName = displayName(uri) ?: "database.pgn"
        val size = fileSize(uri)
        _entries.value.firstOrNull { it.location == uri.toString() || sameFile(it, fileName, size) }?.let {
            // The entry keeps its own way to the file; the new permission is not needed.
            if (it.location != uri.toString()) {
                runCatching { context.contentResolver.releasePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
            }
            open.remove(it.id)?.close()
            return it
        }
        val entry = DbEntry(
            id = UUID.randomUUID().toString(),
            name = fileName.substringBeforeLast('.'),
            source = Source.File,
            location = uri.toString(),
            fileName = fileName,
            size = size,
        )
        update { it + entry }
        return entry
    }

    fun rename(id: String, name: String) = update { list -> list.map { if (it.id == id) it.copy(name = name, renamed = true) else it } }

    /// Takes the database off the list (the file stays where it is). Returns its place,
    /// for "Undo"; the access to the file is kept until [forget].
    fun remove(id: String): Pair<Int, DbEntry>? {
        val list = _entries.value
        val i = list.indexOfFirst { it.id == id }
        if (i < 0) return null
        val e = list[i]
        pending += e
        update { it.filterNot { x -> x.id == id } }
        open.remove(id)?.close()
        return i to e
    }

    /// Gives the access to a removed file back to the system, once "Undo" is gone.
    fun forget(entry: DbEntry) {
        pending.remove(entry)
        if (entry.source == Source.Sample) return
        // The permission of a folder is given back with the last of its databases, also
        // counting those whose "Undo" is still offered.
        val granted = entry.tree ?: entry.location
        if ((_entries.value + pending).any { (it.tree ?: it.location) == granted }) return
        runCatching {
            context.contentResolver.releasePersistableUriPermission(Uri.parse(granted), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }

    /// Keeps the read permission of a picked folder, for the databases added from it
    /// (given back by [forgetFolder] when none is).
    fun keepFolder(tree: Uri) {
        runCatching {
            context.contentResolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }

    /// Looks for databases in a picked folder and in the folders inside it (two levels
    /// down): PGN files, archives, ChessBase archives (.cbv) and ChessBase databases.
    suspend fun scanFolder(tree: Uri): FolderScan = withContext(Dispatchers.IO) {
        val rootId = DocumentsContract.getTreeDocumentId(tree)
        val rootName = childName(DocumentsContract.buildDocumentUriUsingTree(tree, rootId)) ?: ""
        val list = _entries.value
        val found = ArrayList<Found>()
        fun visit(dirId: String, path: String, depth: Int) {
            val children = children(tree, dirId)
            val names = children.filter { !it.dir }.map { it.name }.toSet()
            fun has(name: String) = names.any { it.equals(name, ignoreCase = true) }
            for (c in children.sortedWith { a, b -> naturalOrder(a.name, b.name) }) {
                if (c.dir) continue
                val ext = c.name.substringAfterLast('.', "").lowercase()
                val stem = c.name.substringBeforeLast('.')
                when (ext) {
                    "pgn", "zip", "7z", "cbv" -> {
                        // A ChessBase archive beside its unpacked database is the same
                        // database: the unpacked one is offered.
                        if (ext == "cbv" && has("$stem.cbh") && has("$stem.cbg")) continue
                        val uri = DocumentsContract.buildDocumentUriUsingTree(tree, c.id).toString()
                        val added = list.any { it.location == uri || sameFile(it, c.name, c.size) }
                        found += Found(stem, c.name, uri, Source.File, path, size = c.size, added = added)
                    }
                    "cbh" -> {
                        val dir = DocumentsContract.buildDocumentUriUsingTree(tree, dirId).toString()
                        found += Found(
                            stem, c.name, dir, Source.ChessBase, path,
                            incomplete = !has("$stem.cbg"),
                            added = list.any { it.source == Source.ChessBase && it.location == dir && it.fileName == c.name },
                        )
                    }
                }
            }
            if (depth < 2) {
                for (c in children.filter { it.dir }.sortedWith { a, b -> naturalOrder(a.name, b.name) }) {
                    visit(c.id, if (path.isEmpty()) c.name else "$path/${c.name}", depth + 1)
                }
            }
        }
        visit(rootId, "", 0)
        FolderScan(tree, rootName, found)
    }

    /// Gives a scanned folder's permission back when none of its databases is on the list.
    fun forgetFolder(tree: Uri) {
        if ((_entries.value + pending).any { it.tree == tree.toString() }) return
        runCatching {
            context.contentResolver.releasePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }

    /// Puts the chosen databases of a folder on the list, at its end.
    fun addFound(tree: Uri, chosen: List<Found>) = update { list ->
        list + chosen.filter { !it.added && !it.incomplete }.map {
            DbEntry(
                id = UUID.randomUUID().toString(),
                name = it.name,
                source = it.source,
                location = it.location,
                fileName = it.fileName,
                tree = tree.toString(),
                size = it.size,
            )
        }
    }

    private class Child(val id: String, val name: String, val dir: Boolean, val size: Long?)

    private fun children(tree: Uri, dirId: String): List<Child> {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, dirId)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        return runCatching {
            context.contentResolver.query(uri, columns, null, null, null)?.use { c ->
                buildList {
                    while (c.moveToNext()) {
                        val name = c.getString(1) ?: continue
                        val size = if (c.isNull(3)) null else c.getLong(3)
                        add(Child(c.getString(0), name, c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR, size))
                    }
                }
            }
        }.getOrNull() ?: emptyList()
    }

    private fun childName(uri: Uri): String? = runCatching {
        context.contentResolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    }.getOrNull()

    /// A file for reading. A file deleted from a picked folder makes the provider throw
    /// IllegalArgumentException ("is not a child of the tree"): it is a missing file.
    private fun openDocument(uri: Uri): android.os.ParcelFileDescriptor = try {
        context.contentResolver.openFileDescriptor(uri, "r") ?: throw java.io.FileNotFoundException(uri.toString())
    } catch (e: IllegalArgumentException) {
        throw java.io.FileNotFoundException(e.message)
    }

    /// The files of a ChessBase database: those beside its `.cbh` with the same name.
    private fun openChessBase(e: DbEntry): Database {
        val tree = Uri.parse(e.tree ?: throw ApiException.Io(detail = "no access to the folder"))
        val dirId = DocumentsContract.getDocumentId(Uri.parse(e.location))
        val stem = e.fileName.substringBeforeLast('.')
        // Names may differ in case (sets copied from Windows: "Base.CBH", "base.cbg").
        val parts = children(tree, dirId).filter { !it.dir && it.name.substringBeforeLast('.').equals(stem, ignoreCase = true) }
        if (parts.none { it.name.equals(e.fileName, ignoreCase = true) }) throw java.io.FileNotFoundException(e.fileName)
        if (parts.none { it.name.substringAfterLast('.').equals("cbg", ignoreCase = true) }) throw MissingPart("$stem.cbg")
        // All opened first: a file that cannot be opened leaves none of them open.
        val opened = ArrayList<Pair<String, android.os.ParcelFileDescriptor>>()
        try {
            for (c in parts) {
                val pfd = openDocument(DocumentsContract.buildDocumentUriUsingTree(tree, c.id))
                opened += c.name to pfd
            }
        } catch (t: Throwable) {
            opened.forEach { it.second.close() }
            throw t
        }
        // The descriptors go over to the core, which closes them.
        return openChessbase(e.fileName, opened.map { (name, pfd) -> FolderFile(name, pfd.detachFd()) })
    }

    fun restore(place: Int, entry: DbEntry) {
        pending.remove(entry)
        update { it.toMutableList().apply { add(place.coerceIn(0, size), entry) } }
    }

    /// The same file on the list, picked through another way (its URI differs).
    private fun sameFile(e: DbEntry, fileName: String, size: Long?) =
        e.source == Source.File && size != null && e.size == size && e.fileName == fileName

    private fun fileSize(uri: Uri): Long? = runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst() && !c.isNull(0)) c.getLong(0) else null
        }
    }.getOrNull()

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
                    Source.File -> openFile(e.fileName, openDocument(Uri.parse(e.location)).detachFd())
                    Source.ChessBase -> openChessBase(e)
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
