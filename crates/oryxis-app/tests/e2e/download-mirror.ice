viewport: 1200x750
mode: Zen
-----
# J1: the download-mirror block in Settings > Advanced. Pick Custom,
# an http:// URL is rejected with the inline error, a valid https://
# URL commits and persists. The pick_list dropdown rows live in an
# overlay the text selector cannot see, hence the coordinate clicks
# (stable under the fixed viewport).
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (1175, 64)
expect "Advanced"
click "Advanced"
expect "Download mirror"
click (1048, 103)
settle 300
click (1048, 226)
settle 300
click (410, 187)
type "http://not-safe.example"
click "Save"
expect "Enter a valid https:// URL"
click (410, 187)
type ctrl+a
type "https://mirror.example.cn"
click "Save"
settle 300
screenshot download-mirror-saved
expect "Download mirror"
