//! UI translations. The English text is the key; missing entries fall back to English.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    #[default]
    En,
    Ru,
    Fr,
    De,
    Es,
    It,
}

impl Lang {
    pub const ALL: [Lang; 6] = [Lang::En, Lang::Ru, Lang::Fr, Lang::De, Lang::Es, Lang::It];

    /// Native language name.
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
            Lang::Es => "Español",
            Lang::It => "Italiano",
        }
    }

    fn column(self) -> usize {
        self as usize
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

fn current() -> Lang {
    Lang::ALL
        .get(CURRENT.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or_default()
}

fn table() -> &'static HashMap<&'static str, &'static [&'static str; 6]> {
    static MAP: OnceLock<HashMap<&'static str, &'static [&'static str; 6]>> = OnceLock::new();
    MAP.get_or_init(|| ROWS.iter().map(|row| (row[0], row)).collect())
}

/// Translates an English UI string into the current language.
pub fn tr(key: &'static str) -> &'static str {
    let lang = current();
    if lang == Lang::En {
        return key;
    }
    table()
        .get(key)
        .map(|row| row[lang.column()])
        .unwrap_or(key)
}

/// Translates and substitutes `{name}` placeholders.
pub fn trf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut s = tr(key).to_string();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), &value.to_string());
    }
    s
}

// [English, Русский, Français, Deutsch, Español, Italiano]
const ROWS: &[[&str; 6]] = &[
    // --- board themes ---
    [
        "Tournament",
        "Турнирная",
        "Tournoi",
        "Turnier",
        "Torneo",
        "Torneo",
    ],
    ["Sand", "Песочная", "Sable", "Sand", "Arena", "Sabbia"],
    ["Coral", "Морская", "Corail", "Koralle", "Coral", "Corallo"],
    ["Wood", "Дерево", "Bois", "Holz", "Madera", "Legno"],
    ["Maple", "Клён", "Érable", "Ahorn", "Arce", "Acero"],
    ["Leather", "Кожа", "Cuir", "Leder", "Cuero", "Pelle"],
    ["Marble", "Мрамор", "Marbre", "Marmor", "Mármol", "Marmo"],
    [
        "Blue marble",
        "Синий мрамор",
        "Marbre bleu",
        "Blauer Marmor",
        "Mármol azul",
        "Marmo blu",
    ],
    ["Canvas", "Холст", "Toile", "Leinwand", "Lienzo", "Tela"],
    ["Metal", "Металл", "Métal", "Metall", "Metal", "Metallo"],
    ["Olive", "Олива", "Olive", "Olive", "Oliva", "Oliva"],
    ["Stone", "Камень", "Pierre", "Stein", "Piedra", "Pietra"],
    [
        "Green plastic",
        "Зелёный пластик",
        "Plastique vert",
        "Grünes Plastik",
        "Plástico verde",
        "Plastica verde",
    ],
    [
        "Purple diagonal",
        "Фиолетовые диагонали",
        "Diagonales violettes",
        "Lila Diagonalen",
        "Diagonales moradas",
        "Diagonali viola",
    ],
    [
        "Pink pyramid",
        "Розовые пирамиды",
        "Pyramides roses",
        "Rosa Pyramiden",
        "Pirámides rosas",
        "Piramidi rosa",
    ],
    // --- common / window ---
    [
        "Close the application",
        "Закрыть программу",
        "Fermer l'application",
        "Programm schließen",
        "Cerrar la aplicación",
        "Chiudi l'applicazione",
    ],
    [
        "Cancel",
        "Отмена",
        "Annuler",
        "Abbrechen",
        "Cancelar",
        "Annulla",
    ],
    [
        "Retry",
        "Повторить",
        "Réessayer",
        "Erneut versuchen",
        "Reintentar",
        "Riprova",
    ],
    [
        "Remove from list",
        "Убрать из списка",
        "Retirer de la liste",
        "Aus der Liste entfernen",
        "Quitar de la lista",
        "Rimuovi dall'elenco",
    ],
    ["White", "Белые", "Blancs", "Weiß", "Blancas", "Bianco"],
    ["Black", "Чёрные", "Noirs", "Schwarz", "Negras", "Nero"],
    // --- notation ---
    [
        "Make main line",
        "Сделать главным продолжением",
        "Promouvoir en ligne principale",
        "Zur Hauptvariante machen",
        "Convertir en línea principal",
        "Rendi linea principale",
    ],
    [
        "Delete move and continuation",
        "Удалить ход и продолжение",
        "Supprimer le coup et la suite",
        "Zug und Fortsetzung löschen",
        "Eliminar jugada y continuación",
        "Elimina mossa e seguito",
    ],
    [
        "PGN fragment that could not be parsed; kept as is",
        "Фрагмент PGN, который не удалось разобрать; сохраняется как есть",
        "Fragment PGN non analysable ; conservé tel quel",
        "Nicht lesbares PGN-Fragment; bleibt unverändert erhalten",
        "Fragmento PGN no reconocido; se conserva tal cual",
        "Frammento PGN non interpretabile; conservato così com'è",
    ],
    // --- assets ---
    [
        "Brown",
        "Коричневая",
        "Marron",
        "Braun",
        "Marrón",
        "Marrone",
    ],
    ["Blue", "Синяя", "Bleu", "Blau", "Azul", "Blu"],
    ["Green", "Зелёная", "Vert", "Grün", "Verde", "Verde"],
    ["Grey", "Серая", "Gris", "Grau", "Gris", "Grigio"],
    ["Purple", "Фиолетовая", "Violet", "Lila", "Morado", "Viola"],
    [
        "Piano",
        "Фортепиано",
        "Piano",
        "Klavier",
        "Piano",
        "Pianoforte",
    ],
    // --- app ---
    [
        "Open PGN",
        "Открыть PGN",
        "Ouvrir un PGN",
        "PGN öffnen",
        "Abrir PGN",
        "Apri PGN",
    ],
    [
        "All files",
        "Все файлы",
        "Tous les fichiers",
        "Alle Dateien",
        "Todos los archivos",
        "Tutti i file",
    ],
    [
        "Save game",
        "Сохранить партию",
        "Enregistrer la partie",
        "Partie speichern",
        "Guardar partida",
        "Salva partita",
    ],
    [
        "Choose the UCI engine executable",
        "Выберите исполняемый файл UCI-движка",
        "Choisissez l'exécutable du moteur UCI",
        "UCI-Engine-Programmdatei wählen",
        "Elija el ejecutable del motor UCI",
        "Scegli l'eseguibile del motore UCI",
    ],
    [
        "Engine “{name}” added",
        "Движок «{name}» добавлен",
        "Moteur « {name} » ajouté",
        "Engine „{name}“ hinzugefügt",
        "Motor «{name}» añadido",
        "Motore «{name}» aggiunto",
    ],
    [
        "Not a UCI engine or it failed to start: {e}",
        "Это не UCI-движок или он не запустился: {e}",
        "Ce n'est pas un moteur UCI ou il n'a pas démarré : {e}",
        "Keine UCI-Engine oder Start fehlgeschlagen: {e}",
        "No es un motor UCI o no se pudo iniciar: {e}",
        "Non è un motore UCI o non si è avviato: {e}",
    ],
    [
        "An open database cannot be overwritten with a single game — choose another file",
        "Нельзя перезаписать открытую базу одной партией — выберите другой файл",
        "Impossible d'écraser une base ouverte par une seule partie — choisissez un autre fichier",
        "Eine geöffnete Datenbank kann nicht mit einer einzelnen Partie überschrieben werden — andere Datei wählen",
        "No se puede sobrescribir una base abierta con una sola partida: elija otro archivo",
        "Non si può sovrascrivere un database aperto con una sola partita: scegli un altro file",
    ],
    [
        "Saved: {path}",
        "Сохранено: {path}",
        "Enregistré : {path}",
        "Gespeichert: {path}",
        "Guardado: {path}",
        "Salvato: {path}",
    ],
    [
        "Save failed: {e}",
        "Ошибка сохранения: {e}",
        "Échec de l'enregistrement : {e}",
        "Speichern fehlgeschlagen: {e}",
        "Error al guardar: {e}",
        "Salvataggio non riuscito: {e}",
    ],
    [
        "Could not read the game",
        "Не удалось прочитать партию",
        "Impossible de lire la partie",
        "Partie konnte nicht gelesen werden",
        "No se pudo leer la partida",
        "Impossibile leggere la partita",
    ],
    [
        "Add a UCI engine (for example, Stockfish)",
        "Добавьте UCI-движок (например, Stockfish)",
        "Ajoutez un moteur UCI (par exemple Stockfish)",
        "Fügen Sie eine UCI-Engine hinzu (z. B. Stockfish)",
        "Añada un motor UCI (por ejemplo, Stockfish)",
        "Aggiungi un motore UCI (ad esempio Stockfish)",
    ],
    [
        "The clipboard contains no recognizable PGN game",
        "В буфере обмена нет распознаваемой партии PGN",
        "Le presse-papiers ne contient aucune partie PGN reconnaissable",
        "Die Zwischenablage enthält keine erkennbare PGN-Partie",
        "El portapapeles no contiene ninguna partida PGN reconocible",
        "Gli appunti non contengono una partita PGN riconoscibile",
    ],
    [
        "The clipboard has {count} games — the first one is open. To work with all of them, save them to a .pgn file and add it as a database",
        "В буфере {count} партий — открыта первая. Чтобы работать со всеми, сохраните их в .pgn и добавьте как базу",
        "Le presse-papiers contient {count} parties — la première est ouverte. Pour toutes les utiliser, enregistrez-les dans un fichier .pgn et ajoutez-le comme base",
        "Die Zwischenablage enthält {count} Partien — die erste ist geöffnet. Um alle zu nutzen, speichern Sie sie als .pgn und fügen Sie sie als Datenbank hinzu",
        "El portapapeles tiene {count} partidas; se abrió la primera. Para usarlas todas, guárdelas en un .pgn y añádalo como base",
        "Negli appunti ci sono {count} partite: è aperta la prima. Per usarle tutte, salvale in un file .pgn e aggiungilo come database",
    ],
    [
        "The clipboard is empty or unavailable",
        "Буфер обмена пуст или недоступен",
        "Le presse-papiers est vide ou indisponible",
        "Die Zwischenablage ist leer oder nicht verfügbar",
        "El portapapeles está vacío o no disponible",
        "Gli appunti sono vuoti o non disponibili",
    ],
    [
        "Game modified",
        "Партия изменена",
        "Partie modifiée",
        "Partie geändert",
        "Partida modificada",
        "Partita modificata",
    ],
    [
        "The source PGN is not changed. Save the changes to a separate file?",
        "Исходный PGN не меняется. Сохранить изменения в отдельный файл?",
        "Le PGN source n'est pas modifié. Enregistrer les modifications dans un fichier séparé ?",
        "Die Quell-PGN wird nicht verändert. Änderungen in eine separate Datei speichern?",
        "El PGN original no se modifica. ¿Guardar los cambios en un archivo aparte?",
        "Il PGN originale non viene modificato. Salvare le modifiche in un file separato?",
    ],
    [
        "Save as…",
        "Сохранить как…",
        "Enregistrer sous…",
        "Speichern unter…",
        "Guardar como…",
        "Salva con nome…",
    ],
    [
        "Don't save",
        "Не сохранять",
        "Ne pas enregistrer",
        "Nicht speichern",
        "No guardar",
        "Non salvare",
    ],
    // --- analysis ---
    [
        "The engine hung and was stopped",
        "Движок завис и был остановлен",
        "Le moteur ne répondait plus et a été arrêté",
        "Die Engine reagierte nicht und wurde beendet",
        "El motor se bloqueó y se detuvo",
        "Il motore si è bloccato ed è stato fermato",
    ],
    [
        "The engine exited ({s})",
        "Движок завершился ({s})",
        "Le moteur s'est arrêté ({s})",
        "Die Engine wurde beendet ({s})",
        "El motor terminó ({s})",
        "Il motore è terminato ({s})",
    ],
    [
        "The engine exited",
        "Движок завершился",
        "Le moteur s'est arrêté",
        "Die Engine wurde beendet",
        "El motor terminó",
        "Il motore è terminato",
    ],
    // --- settings ---
    [
        "Settings",
        "Настройки",
        "Paramètres",
        "Einstellungen",
        "Ajustes",
        "Impostazioni",
    ],
    [
        "Engines",
        "Движки",
        "Moteurs",
        "Engines",
        "Motores",
        "Motori",
    ],
    [
        "Board & pieces",
        "Доска и фигуры",
        "Échiquier et pièces",
        "Brett und Figuren",
        "Tablero y piezas",
        "Scacchiera e pezzi",
    ],
    ["Sound", "Звук", "Son", "Ton", "Sonido", "Audio"],
    [
        "Interface",
        "Интерфейс",
        "Interface",
        "Oberfläche",
        "Interfaz",
        "Interfaccia",
    ],
    [
        "Language:",
        "Язык:",
        "Langue :",
        "Sprache:",
        "Idioma:",
        "Lingua:",
    ],
    ["Theme:", "Тема:", "Thème :", "Design:", "Tema:", "Tema:"],
    ["Dark", "Тёмная", "Sombre", "Dunkel", "Oscuro", "Scuro"],
    ["Light", "Светлая", "Clair", "Hell", "Claro", "Chiaro"],
    [
        "UCI engines. The active one is used for game analysis. Time, lines, threads and memory are set with ⚙ in the analysis panel.",
        "UCI-движки. Активный используется для анализа партии. Время, линии, потоки и память настраиваются кнопкой ⚙ в панели анализа.",
        "Moteurs UCI. Le moteur actif sert à l'analyse. Temps, lignes, threads et mémoire se règlent avec ⚙ dans le panneau d'analyse.",
        "UCI-Engines. Die aktive Engine wird für die Analyse verwendet. Zeit, Varianten, Threads und Speicher stellen Sie mit ⚙ im Analysebereich ein.",
        "Motores UCI. El activo se usa para analizar la partida. El tiempo, las líneas, los hilos y la memoria se ajustan con ⚙ en el panel de análisis.",
        "Motori UCI. Quello attivo viene usato per l'analisi. Tempo, linee, thread e memoria si impostano con ⚙ nel pannello di analisi.",
    ],
    [
        "Make active",
        "Сделать активным",
        "Rendre actif",
        "Aktivieren",
        "Activar",
        "Rendi attivo",
    ],
    ["Threads", "Потоки", "Threads", "Threads", "Hilos", "Thread"],
    [
        "+ Add engine…",
        "+ Добавить движок…",
        "+ Ajouter un moteur…",
        "+ Engine hinzufügen…",
        "+ Añadir motor…",
        "+ Aggiungi motore…",
    ],
    [
        "Checking engine…",
        "Проверка движка…",
        "Vérification du moteur…",
        "Engine wird geprüft…",
        "Comprobando el motor…",
        "Verifica del motore…",
    ],
    [
        "Piece set",
        "Набор фигур",
        "Jeu de pièces",
        "Figurensatz",
        "Juego de piezas",
        "Set di pezzi",
    ],
    [
        "Board",
        "Доска",
        "Échiquier",
        "Brett",
        "Tablero",
        "Scacchiera",
    ],
    [
        "Show coordinates",
        "Показывать координаты",
        "Afficher les coordonnées",
        "Koordinaten anzeigen",
        "Mostrar coordenadas",
        "Mostra coordinate",
    ],
    [
        "Show engine best-move arrows",
        "Показывать стрелки лучших ходов движка",
        "Afficher les flèches des meilleurs coups",
        "Pfeile für beste Engine-Züge anzeigen",
        "Mostrar flechas de las mejores jugadas",
        "Mostra frecce delle mosse migliori",
    ],
    [
        "Move sounds",
        "Звуки ходов",
        "Sons des coups",
        "Zuggeräusche",
        "Sonidos de jugadas",
        "Suoni delle mosse",
    ],
    ["Set:", "Набор:", "Jeu :", "Satz:", "Conjunto:", "Set:"],
    [
        "Volume:",
        "Громкость:",
        "Volume :",
        "Lautstärke:",
        "Volumen:",
        "Volume:",
    ],
    [
        "▶ Test",
        "▶ Проверить",
        "▶ Tester",
        "▶ Testen",
        "▶ Probar",
        "▶ Prova",
    ],
    // --- library ---
    [
        "Loading cancelled",
        "Загрузка отменена",
        "Chargement annulé",
        "Laden abgebrochen",
        "Carga cancelada",
        "Caricamento annullato",
    ],
    [
        "Databases",
        "Базы",
        "Bases",
        "Datenbanken",
        "Bases",
        "Database",
    ],
    [
        "⚙ Settings",
        "⚙ Настройки",
        "⚙ Paramètres",
        "⚙ Einstellungen",
        "⚙ Ajustes",
        "⚙ Impostazioni",
    ],
    [
        "📋 Paste PGN",
        "📋 Вставить PGN",
        "📋 Coller un PGN",
        "📋 PGN einfügen",
        "📋 Pegar PGN",
        "📋 Incolla PGN",
    ],
    [
        "Open a game from the clipboard",
        "Открыть партию из буфера обмена",
        "Ouvrir une partie depuis le presse-papiers",
        "Partie aus der Zwischenablage öffnen",
        "Abrir una partida del portapapeles",
        "Apri una partita dagli appunti",
    ],
    [
        "♟ New game",
        "♟ Новая партия",
        "♟ Nouvelle partie",
        "♟ Neue Partie",
        "♟ Nueva partida",
        "♟ Nuova partita",
    ],
    [
        "Empty board in the starting position (Ctrl+N)",
        "Пустая доска в начальной позиции (Ctrl+N)",
        "Échiquier en position initiale (Ctrl+N)",
        "Brett in der Grundstellung (Ctrl+N)",
        "Tablero en la posición inicial (Ctrl+N)",
        "Scacchiera nella posizione iniziale (Ctrl+N)",
    ],
    [
        "+ Add PGN",
        "+ Добавить PGN",
        "+ Ajouter un PGN",
        "+ PGN hinzufügen",
        "+ Añadir PGN",
        "+ Aggiungi PGN",
    ],
    [
        "Open a PGN file (Ctrl+O)",
        "Открыть PGN-файл (Ctrl+O)",
        "Ouvrir un fichier PGN (Ctrl+O)",
        "PGN-Datei öffnen (Ctrl+O)",
        "Abrir un archivo PGN (Ctrl+O)",
        "Apri un file PGN (Ctrl+O)",
    ],
    [
        "{n} games",
        "{n} партий",
        "{n} parties",
        "{n} Partien",
        "{n} partidas",
        "{n} partite",
    ],
    [
        "loading… {n}",
        "загрузка… {n}",
        "chargement… {n}",
        "lädt… {n}",
        "cargando… {n}",
        "caricamento… {n}",
    ],
    ["error", "ошибка", "erreur", "Fehler", "error", "errore"],
    [
        "file not found",
        "файл не найден",
        "fichier introuvable",
        "Datei nicht gefunden",
        "archivo no encontrado",
        "file non trovato",
    ],
    [
        "not loaded",
        "не загружена",
        "non chargée",
        "nicht geladen",
        "no cargada",
        "non caricato",
    ],
    [
        "Reload file",
        "Перечитать файл",
        "Recharger le fichier",
        "Datei neu laden",
        "Volver a leer el archivo",
        "Ricarica il file",
    ],
    [
        "Choose a database above, add a PGN file or start a new game",
        "Выберите базу сверху, добавьте PGN-файл или начните новую партию",
        "Choisissez une base ci-dessus, ajoutez un fichier PGN ou commencez une nouvelle partie",
        "Wählen Sie oben eine Datenbank, fügen Sie eine PGN-Datei hinzu oder beginnen Sie eine neue Partie",
        "Elija una base arriba, añada un archivo PGN o empiece una partida nueva",
        "Scegli un database in alto, aggiungi un file PGN o inizia una nuova partita",
    ],
    [
        "Reading {path} — games found: {n}",
        "Чтение {path} — найдено партий: {n}",
        "Lecture de {path} — parties trouvées : {n}",
        "Lese {path} — gefundene Partien: {n}",
        "Leyendo {path}: partidas encontradas: {n}",
        "Lettura di {path}: partite trovate: {n}",
    ],
    [
        "Could not open {path}: {e}",
        "Не удалось открыть {path}: {e}",
        "Impossible d'ouvrir {path} : {e}",
        "{path} konnte nicht geöffnet werden: {e}",
        "No se pudo abrir {path}: {e}",
        "Impossibile aprire {path}: {e}",
    ],
    [
        "· {n} games · encoding {enc}",
        "· {n} партий · кодировка {enc}",
        "· {n} parties · encodage {enc}",
        "· {n} Partien · Kodierung {enc}",
        "· {n} partidas · codificación {enc}",
        "· {n} partite · codifica {enc}",
    ],
    [
        "🔍 Search: player, event, ECO, year",
        "🔍 Поиск: игрок, турнир, ECO, год",
        "🔍 Recherche : joueur, tournoi, ECO, année",
        "🔍 Suche: Spieler, Turnier, ECO, Jahr",
        "🔍 Buscar: jugador, torneo, ECO, año",
        "🔍 Cerca: giocatore, torneo, ECO, anno",
    ],
    [
        "Result",
        "Результат",
        "Résultat",
        "Ergebnis",
        "Resultado",
        "Risultato",
    ],
    ["Moves", "Ходы", "Coups", "Züge", "Jugadas", "Mosse"],
    ["Event", "Турнир", "Tournoi", "Turnier", "Torneo", "Torneo"],
    ["Date", "Дата", "Date", "Datum", "Fecha", "Data"],
    // --- game view ---
    [
        "PGN compatibility",
        "Совместимость PGN",
        "Compatibilité PGN",
        "PGN-Kompatibilität",
        "Compatibilidad PGN",
        "Compatibilità PGN",
    ],
    [
        "line {n}: ",
        "строка {n}: ",
        "ligne {n} : ",
        "Zeile {n}: ",
        "línea {n}: ",
        "riga {n}: ",
    ],
    [
        "⬅ Library",
        "⬅ Библиотека",
        "⬅ Bibliothèque",
        "⬅ Bibliothek",
        "⬅ Biblioteca",
        "⬅ Libreria",
    ],
    [
        "Back to the game list (Ctrl+W)",
        "Вернуться к списку партий (Ctrl+W)",
        "Retour à la liste des parties (Ctrl+W)",
        "Zurück zur Partienliste (Ctrl+W)",
        "Volver a la lista de partidas (Ctrl+W)",
        "Torna all'elenco delle partite (Ctrl+W)",
    ],
    [
        "PGN compatibility notes",
        "Замечания по совместимости PGN",
        "Remarques de compatibilité PGN",
        "Hinweise zur PGN-Kompatibilität",
        "Avisos de compatibilidad PGN",
        "Note di compatibilità PGN",
    ],
    [
        "Move not added: {e}",
        "Ход не добавлен: {e}",
        "Coup non ajouté : {e}",
        "Zug nicht hinzugefügt: {e}",
        "Jugada no añadida: {e}",
        "Mossa non aggiunta: {e}",
    ],
    [
        "Analysis: on",
        "Анализ: вкл",
        "Analyse : activée",
        "Analyse: an",
        "Análisis: activado",
        "Analisi: attiva",
    ],
    [
        "Analysis: off",
        "Анализ: выкл",
        "Analyse : désactivée",
        "Analyse: aus",
        "Análisis: desactivado",
        "Analisi: disattiva",
    ],
    [
        "Turn the engine on/off (L)",
        "Включить/выключить движок (L)",
        "Activer/désactiver le moteur (L)",
        "Engine ein/aus (L)",
        "Activar/desactivar el motor (L)",
        "Attiva/disattiva il motore (L)",
    ],
    [
        "No engine selected",
        "Движок не выбран",
        "Aucun moteur sélectionné",
        "Keine Engine gewählt",
        "Ningún motor seleccionado",
        "Nessun motore selezionato",
    ],
    [
        "starting…",
        "запуск…",
        "démarrage…",
        "startet…",
        "iniciando…",
        "avvio…",
    ],
    [
        " · done",
        " · готово",
        " · terminé",
        " · fertig",
        " · listo",
        " · completato",
    ],
    [
        " · {n} Mn/s",
        " · {n} Музл/с",
        " · {n} Mn/s",
        " · {n} MKn/s",
        " · {n} Mn/s",
        " · {n} Mn/s",
    ],
    [
        "Depth {n}",
        "Глубина {n}",
        "Profondeur {n}",
        "Tiefe {n}",
        "Profundidad {n}",
        "Profondità {n}",
    ],
    [
        "Engine settings",
        "Настройки движка",
        "Réglages du moteur",
        "Engine-Einstellungen",
        "Ajustes del motor",
        "Impostazioni del motore",
    ],
    [
        "Engine line preview",
        "Предпросмотр линии движка",
        "Aperçu de la ligne du moteur",
        "Vorschau der Engine-Variante",
        "Vista previa de la línea del motor",
        "Anteprima della linea del motore",
    ],
    [
        "move {a}/{b}",
        "ход {a}/{b}",
        "coup {a}/{b}",
        "Zug {a}/{b}",
        "jugada {a}/{b}",
        "mossa {a}/{b}",
    ],
    [
        "Add as variation",
        "Добавить как вариант",
        "Ajouter comme variante",
        "Als Variante hinzufügen",
        "Añadir como variante",
        "Aggiungi come variante",
    ],
    [
        "Exit (Esc)",
        "Выйти (Esc)",
        "Quitter (Échap)",
        "Beenden (Esc)",
        "Salir (Esc)",
        "Esci (Esc)",
    ],
    [
        "Line added as a variation (Ctrl+Z to undo)",
        "Линия добавлена как вариант (Ctrl+Z — отменить)",
        "Ligne ajoutée comme variante (Ctrl+Z pour annuler)",
        "Variante hinzugefügt (Ctrl+Z zum Rückgängigmachen)",
        "Línea añadida como variante (Ctrl+Z para deshacer)",
        "Linea aggiunta come variante (Ctrl+Z per annullare)",
    ],
    [
        "Could not add the line: {e}",
        "Не удалось добавить линию: {e}",
        "Impossible d'ajouter la ligne : {e}",
        "Variante konnte nicht hinzugefügt werden: {e}",
        "No se pudo añadir la línea: {e}",
        "Impossibile aggiungere la linea: {e}",
    ],
    [
        "To start (Home)",
        "В начало (Home)",
        "Au début (Début)",
        "Zum Anfang (Pos1)",
        "Al inicio (Inicio)",
        "All'inizio (Home)",
    ],
    [
        "Back (←)",
        "Назад (←)",
        "Reculer (←)",
        "Zurück (←)",
        "Atrás (←)",
        "Indietro (←)",
    ],
    [
        "Forward (→)",
        "Вперёд (→)",
        "Avancer (→)",
        "Vor (→)",
        "Adelante (→)",
        "Avanti (→)",
    ],
    [
        "To the end of the line (End)",
        "В конец линии (End)",
        "À la fin de la ligne (Fin)",
        "Zum Ende der Variante (Ende)",
        "Al final de la línea (Fin)",
        "Alla fine della linea (Fine)",
    ],
    [
        "Flip board (F)",
        "Перевернуть доску (F)",
        "Retourner l'échiquier (F)",
        "Brett drehen (F)",
        "Girar el tablero (F)",
        "Ruota la scacchiera (F)",
    ],
    [
        "Undo move/edit (Ctrl+Z)",
        "Отменить изменение (Ctrl+Z)",
        "Annuler la modification (Ctrl+Z)",
        "Änderung rückgängig (Ctrl+Z)",
        "Deshacer cambio (Ctrl+Z)",
        "Annulla modifica (Ctrl+Z)",
    ],
    [
        "Redo move/edit (Ctrl+Y)",
        "Вернуть изменение (Ctrl+Y)",
        "Rétablir la modification (Ctrl+Y)",
        "Änderung wiederherstellen (Ctrl+Y)",
        "Rehacer cambio (Ctrl+Y)",
        "Ripristina modifica (Ctrl+Y)",
    ],
    [
        "Copy PGN",
        "Копировать PGN",
        "Copier le PGN",
        "PGN kopieren",
        "Copiar PGN",
        "Copia PGN",
    ],
    [
        "PGN copied",
        "PGN скопирован",
        "PGN copié",
        "PGN kopiert",
        "PGN copiado",
        "PGN copiato",
    ],
    [
        "Copy FEN",
        "Копировать FEN",
        "Copier la FEN",
        "FEN kopieren",
        "Copiar FEN",
        "Copia FEN",
    ],
    [
        "FEN copied",
        "FEN скопирован",
        "FEN copiée",
        "FEN kopiert",
        "FEN copiado",
        "FEN copiato",
    ],
    [
        "Save game as…",
        "Сохранить партию как…",
        "Enregistrer la partie sous…",
        "Partie speichern unter…",
        "Guardar partida como…",
        "Salva partita con nome…",
    ],
    [
        "Compatibility notes",
        "Замечания по совместимости",
        "Remarques de compatibilité",
        "Kompatibilitätshinweise",
        "Avisos de compatibilidad",
        "Note di compatibilità",
    ],
    [
        "Copy PGN (Ctrl+Shift+C)",
        "Копировать PGN (Ctrl+Shift+C)",
        "Copier le PGN (Ctrl+Maj+C)",
        "PGN kopieren (Ctrl+Umschalt+C)",
        "Copiar PGN (Ctrl+Mayús+C)",
        "Copia PGN (Ctrl+Maiusc+C)",
    ],
    ["Engine", "Движок", "Moteur", "Engine", "Motor", "Motore"],
    [
        "not selected",
        "не выбран",
        "non sélectionné",
        "nicht gewählt",
        "no seleccionado",
        "non selezionato",
    ],
    [
        "Add or edit engines…",
        "Добавить или изменить движки…",
        "Ajouter ou modifier des moteurs…",
        "Engines hinzufügen oder bearbeiten…",
        "Añadir o editar motores…",
        "Aggiungi o modifica motori…",
    ],
    [
        "Add engine…",
        "Добавить движок…",
        "Ajouter un moteur…",
        "Engine hinzufügen…",
        "Añadir motor…",
        "Aggiungi motore…",
    ],
    [
        "Search time",
        "Время поиска",
        "Temps de recherche",
        "Suchzeit",
        "Tiempo de búsqueda",
        "Tempo di ricerca",
    ],
    ["{n} s", "{n} с", "{n} s", "{n} s", "{n} s", "{n} s"],
    ["Lines", "Линий", "Lignes", "Varianten", "Líneas", "Linee"],
    [
        "Memory",
        "Память",
        "Mémoire",
        "Speicher",
        "Memoria",
        "Memoria",
    ],
    ["{n} GB", "{n} ГБ", "{n} Go", "{n} GB", "{n} GB", "{n} GB"],
    ["{n} MB", "{n} МБ", "{n} Mo", "{n} MB", "{n} MB", "{n} MB"],
    [
        "Checkmate · 0-1",
        "Мат · 0-1",
        "Mat · 0-1",
        "Matt · 0-1",
        "Jaque mate · 0-1",
        "Scacco matto · 0-1",
    ],
    [
        "Checkmate · 1-0",
        "Мат · 1-0",
        "Mat · 1-0",
        "Matt · 1-0",
        "Jaque mate · 1-0",
        "Scacco matto · 1-0",
    ],
    [
        "Stalemate · ½-½",
        "Пат · ½-½",
        "Pat · ½-½",
        "Patt · ½-½",
        "Ahogado · ½-½",
        "Stallo · ½-½",
    ],
    [
        "Insufficient material",
        "Недостаточно материала",
        "Matériel insuffisant",
        "Ungenügendes Material",
        "Material insuficiente",
        "Materiale insufficiente",
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `tr("…")` / `trf("…"` key used in the sources must have a translation row.
    #[test]
    fn all_used_keys_are_translated() {
        let sources = [
            include_str!("app.rs"),
            include_str!("assets.rs"),
            include_str!("analysis.rs"),
            include_str!("game_view.rs"),
            include_str!("library.rs"),
            include_str!("notation.rs"),
            include_str!("settings_ui.rs"),
            include_str!("widgets.rs"),
        ];
        let mut missing = Vec::new();
        let mut found = 0;
        for src in sources {
            for marker in ["tr(", "trf("] {
                for (i, _) in src.match_indices(marker) {
                    // Skip identifiers that merely end in `tr` (e.g. `str(`).
                    let prev = src[..i].chars().next_back();
                    if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    // rustfmt may move the literal to the next line.
                    let rest = src[i + marker.len()..].trim_start();
                    let Some(rest) = rest.strip_prefix('"') else {
                        continue;
                    };
                    let Some(end) = rest.find('"') else { continue };
                    found += 1;
                    let key = &rest[..end];
                    if !table().contains_key(key) {
                        missing.push(key.to_string());
                    }
                }
            }
        }
        assert!(found > 100, "scanner found only {found} keys");
        assert!(missing.is_empty(), "untranslated keys: {missing:?}");
    }

    #[test]
    fn rows_are_unique_and_complete() {
        let mut seen = std::collections::HashSet::new();
        for row in ROWS {
            assert!(seen.insert(row[0]), "duplicate key {}", row[0]);
            assert!(
                row.iter().all(|s| !s.is_empty()),
                "empty cell in {}",
                row[0]
            );
        }
    }
}
