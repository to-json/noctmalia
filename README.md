# noctmalia

## Disclosure

This is me playing with the idea "if vibe-coding works so well,
why aren't we seeing full desktop apps cooked up in a week to 
solve real problems"

email client ui has sucked, a lot, especially on linux, for ages.
this seems like a solvable problem. what if we had an email client
that matched our system shell, had pleasant, modern ui components,
and sought to improve a bit on the email experience with better 
visibility into headers and a vim-inspired browsing experience

well, that's mutt, but, noctmalia also exists. title pending

## What is this doing, anyway

```
noctmalia ──spawns──►  thunderbird --headless ── bridge (MailExtension) ── nm-shim ──►  noctmalia
                       in noctmalia-thunderbird.scope                   NDJSON over a unix socket
                                                                        (the app listens; the shim connects in)
```

| Path | What |
|---|---|
| `flake.nix`, `scripts/` | The devshell, and the cargo/run wrappers around it |
| `crates/noctmalia-bridge` | protocol|
| `crates/noctmalia` | app |
| `bridge/` | MailExtension sideloaded into Thunderbird |
| `tools/` | what it says |
| `docs/ | this directory has been taken over by robots |

## Running it

you're gonna need another repo as a repo peer. this will ship as
dependencies later, but, i didn't feel like sorting it at the time.

clone to-json/noctalia-iced into the same parent. then



```sh
just                  # run it
just fake             # fake it
just dev              # run it with test affordances
just seed --reset     # load test fixtures
just status           # what is running
just shots            # screenshot every surface headlessly
```

right now we download a specific, x86 build of tbird. use just, for now

## Build and run

didn't i just say use just?

ok. i did this on a box with nix but no cargo, so, there's a flake.nix, and, some scripts

```sh
scripts/run.sh                  # build release and run it
scripts/cargo.sh test --workspace
```

potential gotcha: `nix develop` reads from git; add stuff you want available

`tools/noctmalia-ctl.py status` does what you might expect. we have a whole socket ipc, so
you can build mail and calendar based features into, say, your system bootstrap
`NOCTMALIA_TRACE=1` will enable detailed tracing

`scripts/run.sh` defaults to software rendering; `NOCTMALIA_GPU=1` to use the GPU. there
are a few other fun env vars; check the source if you're so inclined

### Against a fake Thunderbird

```sh
scripts/run.sh --backend external      # binds $XDG_RUNTIME_DIR/noctmalia/bridge.sock and waits
tools/fake-bridge.py                   # connects to it and serves mail and contacts from memory
```

`tools/fake-bridge.py --empty` serves an empty store

### Test data in the real thing

```sh
just dev                              # in one terminal: the window, dev methods on
tools/seed.sh                         # in another: seed accounts, contacts and mail
tools/seed.sh --reset                 # ...replacing what is already there
tools/seed.sh --flood 500             # ...plus 500 unremarkable messages
```

## Mail

Aiming for vimtuitive, here. h and l move between panes, j and k within a pane. 
Enter opens the item at the cursor, space moves you into a thread. 
clicking/`n` marks an item read 
`e` to archive 
`d` to delete 
`u` for undo
`r`/`R`/`f` to reply (all) or forward
`S` to make a mail filtering rule from the message at the cursor
`/` to searches. by default it filters loaded mail, press enter to send a query
`g i`, `g s`, `g d`, `g a`, `g t` jump to maildirs (inbox, spam, etc)
`g p` / `g c` switch to people and calendar respectively
commands take a count where it makes sense

### Markdown first

You can technically render html email, because it's tyool 2026 and that's mandatory. However,
we default to the plaintext version where possible, and then pass it through a markdown renderer.
In this way, if you and your friends all use noctmalia, then, email supports markdown.

`o` to render html, but, do notice how often you just don't need to.

### Thunderbird++

We are a parasite riding on thunderbird, because thunderbird is actually very good, except
when you have to like...use it. All our central features operate on the tbird headless api,
and we will strive to support any feature exposed by the api that adds value to the basic usecases

### Errything match

We ship a noctalia template, because we aspire to be the obvious email and calendar suite for a
noctalia user


```sh
noctmalia --install-palette-template   # or: just palette
```

## Status

Under _heavy_ development. Probably not """production ready""". Not my daily driver yet
