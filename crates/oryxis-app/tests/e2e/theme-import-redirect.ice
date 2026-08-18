viewport: 1200x750
mode: Zen
-----
# A theme pasted in the wrong panel is not an error, it is a paste in the
# wrong place: the app carries it over to the importer that handles it
# (the two live in different settings sections) and says so in a toast.
# This walks the case from the field, a Windows Terminal scheme shared in
# discussion #68 pasted into Settings > Interface, which used to dead-end
# on "Not an Oryxis UI theme file".
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"

# Settings (gear) -> Interface -> the App theme row, which sits below the
# fold of a freshly opened section.
click (1175, 64)
settle
click "Interface"
settle
scroll (0, -1400) (700, 400)
settle
click "Oryxis Dark"
settle
click "Import"
settle
expect "Paste an Oryxis UI theme (JSON), or load the file. The colors open in the editor to review and save."

# Paste the terminal scheme into the interface importer and apply.
clipboard "{\n  \"name\": \"Ubuntu\",\n  \"background\": \"#300a24\",\n  \"foreground\": \"#ffffff\",\n  \"cursorColor\": \"#ffffff\",\n  \"black\": \"#2e3436\",\n  \"red\": \"#cc0000\",\n  \"green\": \"#4e9a06\",\n  \"yellow\": \"#c4a000\",\n  \"blue\": \"#3465a4\",\n  \"purple\": \"#75507b\",\n  \"cyan\": \"#06989a\",\n  \"white\": \"#d3d7cf\",\n  \"brightBlack\": \"#555753\",\n  \"brightRed\": \"#ef2929\",\n  \"brightGreen\": \"#8ae234\",\n  \"brightYellow\": \"#fce94f\",\n  \"brightBlue\": \"#729fcf\",\n  \"brightPurple\": \"#ad7fa8\",\n  \"brightCyan\": \"#34e2e2\",\n  \"brightWhite\": \"#eeeeec\"\n}"
click (600, 420)
type ctrl+v
click "Import"

# The toast explains the move; it is asserted before `settle`, whose wait
# for quiescence outlives the chip's own dwell.
expect "That is a terminal color scheme, so the import moved to Terminal Settings."
settle

# What is on screen now is the TERMINAL importer (its own hint), and the
# paste travelled with it: Import parses the scheme and opens the editor,
# which an empty panel could never do.
expect "Paste an iTerm (.itermcolors), Windows Terminal (JSON) or base16 scheme. The colors open in the editor to review and save."
click "Import"
settle
expect "Create custom theme"
