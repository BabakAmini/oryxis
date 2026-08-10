# Backups, and getting them off this machine

Oryxis keeps everything in one encrypted vault file. This page is about
getting a copy of it somewhere else: another disk, a cloud account, a
Telegram chat, whatever you already trust.

There are two mechanisms and they answer different questions. Picking
the wrong one is the main way people end up disappointed, so start
here.

## Which one do you want

| What you want | Use | Why |
|---|---|---|
| The same vault on my laptop and my desktop | **Sync** | Changes flow both ways and merge |
| A copy I can restore from if this machine dies | **Export** | One file, one moment in time, restorable on its own |
| A copy in my cloud drive, kept current | **Sync to a folder** | Your cloud client already syncs that folder |
| A copy on my Nextcloud / Synology, no client installed | **Sync over WebDAV** | Talks to the server directly, and detects a conflict instead of healing it |
| A copy in a Telegram chat / an S3 bucket / anywhere exotic | **Sync to a folder + a tool that watches it** | See [Sending a copy somewhere Oryxis does not go](#sending-a-copy-somewhere-oryxis-does-not-go) |

Oryxis has no scheduler and does not run in the background. Nothing
here fires while the app is closed; that is a deliberate limit, not an
oversight. Anything that must happen on a timer is a job for a tool
your system already runs on a timer, which the last section covers.

## Export: one file you can restore from

Settings > Security & Privacy > Export / Import.

You pick what goes in (hosts, keys, identities, snippets, settings),
you set a password, and you get a single encrypted file. That password
is separate from your vault's, and the file is useless without it: we
cannot recover it for you.

Import reads the same file back on any machine, and it can inspect the
file before importing so you can see what it contains.

There is also **Export to SFTP**, which writes that same file straight
to a host in your vault instead of to local disk.

An export is a snapshot in time. It does not update itself.

## Sync: the same vault in more than one place

Settings > Sync. Sync is off by default and has five transports. Only
one runs at a time.

### Peer-to-peer

The default. Your devices find each other on the local network and talk
directly, end to end encrypted, with no server anywhere. To reach
across networks you run your own `oryxis-relay`; Oryxis hosts none.

Use this whenever the machines are on the same network. It is the
fastest and the only one with no file sitting anywhere.

### SFTP file

One encrypted snapshot on a host from your vault. Each device merges
what is there and writes back.

### Git

The same snapshot, committed to a Git remote: any forge, or a bare
repository on a box you own. This is the only transport that keeps
**history**, so a vault you wrecked (a bad import, a deletion that
synced from the wrong machine) can be read back from an earlier commit.

It drives the `git` you already have installed rather than bundling
one, so `git` has to be on PATH; the setting says so if it is not.
Authentication is git's own: an SSH key your agent already holds, or a
credential helper. Oryxis never prompts for it, and a remote that would
have asked fails fast instead of hanging.

Every round is one commit, and only when something actually changed:
the snapshot is re-encrypted with a fresh nonce each time, so the app
compares a fingerprint of the vault's contents rather than the bytes.

Two things worth knowing. The repository ends up holding every past
version of your vault, all encrypted, which is the feature and also a
reason to keep it private. And the working copy lives in
`~/.oryxis/sync-git/`; it is a cache, safe to delete, and the next sync
re-creates it.

### WebDAV

The same snapshot on a Nextcloud, ownCloud, Synology or plain WebDAV
server. You give it a collection URL, an account and an app password;
there is no desktop client to install and no OAuth app to register
anywhere.

This is the transport with the best conflict behaviour after Git. The
write carries the tag the server gave you when you read, so a server
that has changed since refuses it, and Oryxis redoes the round on top
of what landed instead of flattening it. The folder and SFTP
transports can only heal a collision afterwards.

A URL ending in `/` is a folder and gets the shared snapshot name; a
URL naming a file is used as typed, which is how two sync groups share
one account. If the folder does not exist yet Oryxis creates it once.

### Folder

One encrypted snapshot in a directory your machine already has. This is
the one that quietly covers every cloud provider, because Oryxis does
not talk to any of them: it writes a file, and whatever owns that
folder carries it.

Point it at:

- your **OneDrive**, **Google Drive**, **Dropbox** or **iCloud** folder
- a **network share** (SMB, NFS, a NAS mount)
- a **Syncthing** directory
- an **external disk** or a USB stick
- any plain folder you back up by other means

Set the same **sync passphrase** on every device: it derives the key
the snapshot is encrypted with, so devices that disagree about it are
in different sync groups even when they share a file. The passphrase is
not your vault password and is never sent anywhere.

The snapshot is written to a temporary file in the same folder and then
renamed over the target, so another device never reads half of one.

**One caveat, and it is real.** Two machines writing the same
cloud-mirrored folder at the same time can race: whoever writes last
wins, and the edits from the other one arrive on its next round rather
than being lost. Your cloud client may also leave a file with
"conflicted copy" in the name, which Oryxis will not read. If the two
machines are on the same network, peer-to-peer avoids all of this.

## Sending a copy somewhere Oryxis does not go

Telegram, S3, a Git repository, an rsync target, a tape robot. Oryxis
deliberately integrates with none of these, and it does not need to:
point the folder transport (or an export) at a directory, and use a
tool that already knows how to send files.

This is the same shape as the tmux page: Oryxis produces the artifact,
you choose what carries it. It keeps one file format working with every
destination that exists, instead of a list of integrations that rot.

### Telegram, with a proxy

Two common ways, both around ten lines.

**With rclone**, which speaks a lot of destinations and honours a proxy
through the standard environment variables:

```bash
# One folder, whatever Oryxis last wrote into it.
export ALL_PROXY=socks5://127.0.0.1:1080     # your proxy, if you need one
rclone copy ~/OryxisSync remote:oryxis-backups
```

**With Teldrive**, if you want Telegram to BE the folder rather than a
destination you post to: [Teldrive](https://github.com/tgdrive/teldrive)
stores files in a Telegram channel and plugs into rclone as a backend,
so `rclone mount` gives you a directory the folder transport syncs
into, both ways, and Teldrive itself takes a socks5 or HTTP proxy for
the Telegram side. That makes Telegram the same kind of destination as
the cloud folders above: mounted, kept current, and restorable by
pointing a fresh install at the same folder.

**With a bot and curl**, if Telegram specifically is the destination:

```bash
#!/bin/sh
# send-oryxis-backup.sh
# Bot token and chat id come from @BotFather and your chat.
TOKEN="123456:ABC..."
CHAT="-1001234567890"
SNAPSHOT="$HOME/OryxisSync/oryxis-sync.bin"

curl --socks5-hostname 127.0.0.1:1080 \
     -F "chat_id=${CHAT}" \
     -F "caption=Oryxis vault $(hostname) $(date -Iseconds)" \
     -F "document=@${SNAPSHOT}" \
     "https://api.telegram.org/bot${TOKEN}/sendDocument"
```

Notes that will save you time:

- Swap `--socks5-hostname` for `-x http://host:port` if your proxy is
  HTTP. Drop the flag entirely if you do not need one.
- A self-hosted Bot API server replaces `api.telegram.org` in that URL,
  which is also how you get past the **50 MB** limit the public Bot API
  puts on `sendDocument` (a self-hosted server allows up to 2 GB).
- Run it on a timer with whatever your system already has: `cron`,
  a systemd timer, or Task Scheduler on Windows. That is the part
  Oryxis does not do.

### What you are sending

Both the sync snapshot and the export are encrypted before they leave
Oryxis, so the chat, the bucket or the drive never sees your hosts or
your keys. Whoever holds the passphrase holds the data, which is the
whole reason you can use a destination you do not fully trust.

Keep the passphrase somewhere other than the destination. A backup you
cannot decrypt is not a backup.
