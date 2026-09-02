//! The shell shims: register a completer, forward the command line, act on the sentinel.
//!
//! These replaced ~700 lines of poe-derived shell that each shell carried its own copy of.
//! Everything they used to decide now lives in [`crate::engine`], so the only shell code
//! left is the part shells genuinely do better than Rust: parsing their own syntax, and
//! completing their own filesystem paths.
//!
//! Neither shim invokes `devkit` at load time. That is the whole point: the previous
//! PowerShell registration ran `devkit complete script | Invoke-Expression` from `$PROFILE`
//! at every shell start, which is what made a global devkit install mandatory.

/// Version of the shim *protocol and text*, sent on every request.
///
/// Bump this whenever either constant below changes — a wire-format change and a cosmetic
/// fix both count, because both mean an installed shim differs from the shipped one. It is
/// deliberately unrelated to the package version: an ordinary devkit release that leaves
/// these constants alone changes nothing and triggers no repair.
pub const SHIM_VERSION: u32 = 1;

/// `devkit complete script --bash`; written to the bash completion directories by
/// `devkit complete install --bash`.
pub const BASH: &str = r##"# Bash completion for poe - devkit thin shim (shim version 1)
#
# Installed by `devkit complete install --bash`. All completion logic lives in
# `devkit complete query`; this file only forwards the command line and acts on the
# directory/file sentinel. Rewritten automatically when devkit's shim version changes.

_poe_complete() {
    COMPREPLY=()

    # No devkit on PATH - normally an unactivated venv. Offer nothing rather than erroring:
    # a completer that fails prints over the user's prompt.
    command -v devkit >/dev/null 2>&1 || return 0

    local out
    out=$(devkit complete query --shell bash --shim-version 1 \
            --line "$COMP_LINE" --point "$COMP_POINT" 2>/dev/null) || return 0
    [[ -z "$out" ]] && return 0

    # First line is the directive; the rest, if any, are candidates.
    local header="${out%%$'\n'*}"
    case "$header" in
        dirs)
            # The shell enumerates paths, keeping its own quoting and trailing-slash rules.
            _filedir -d 2>/dev/null || COMPREPLY=($(compgen -d -- "$2"))
            return 0
            ;;
        files)
            _filedir 2>/dev/null || COMPREPLY=($(compgen -f -- "$2"))
            return 0
            ;;
        items) ;;
        *) return 0 ;;
    esac

    # Strip the header. If nothing was stripped there was no newline, so no candidates.
    local body="${out#*$'\n'}"
    [[ "$body" == "$out" ]] && return 0

    # devkit already filtered by prefix, so each first column is inserted verbatim.
    local value
    while IFS=$'\t' read -r value _; do
        [[ -n "$value" ]] && COMPREPLY+=("$value")
    done <<< "$body"
    return 0
}

complete -F _poe_complete poe
"##;

/// `devkit complete script --powershell`; written to
/// `~/.local/share/devkit/poe-completion.ps1` and dot-sourced from `$PROFILE`.
pub const POWERSHELL: &str = r##"# PowerShell completion for poe - devkit thin shim (shim version 1)
#
# Installed by `devkit complete install --powershell`. All completion logic lives in
# `devkit complete query`; this file only forwards the command line and acts on the
# directory/file sentinel. Rewritten automatically when devkit's shim version changes.

Register-ArgumentCompleter -CommandName poe -Native -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    # No devkit on PATH - normally an unactivated venv. Returning nothing is the correct
    # degenerate answer; throwing here would break the prompt.
    $dk = Get-Command devkit -ErrorAction SilentlyContinue
    if (-not $dk) { return }

    $els = @($commandAst.CommandElements)

    # PowerShell measures the cursor in characters into the whole line, while the engine
    # wants an element index. Past the last element means a fresh word is being started.
    $cword = $els.Count
    for ($i = 0; $i -lt $els.Count; $i++) {
        $e = $els[$i].Extent
        if ($cursorPosition -ge $e.StartOffset -and $cursorPosition -le $e.EndOffset) {
            $cword = $i
            break
        }
    }

    # Send the elements PowerShell's own parser produced rather than raw text: they are
    # already correctly split and unquoted, which no external splitter could improve on.
    $texts = @($els | ForEach-Object { $_.Extent.Text })

    # --word-to-complete uses the =value form deliberately: $wordToComplete is empty when a
    # fresh word is starting, and an empty argument passed separately can be dropped
    # entirely, which would make the parser swallow the next token as this flag's value.
    $out = & $dk.Source complete query --shell powershell --shim-version 1 --cword $cword --word-to-complete=$wordToComplete -- @texts 2>$null
    if (-not $out) { return }

    $lines = @($out -split "\r?\n" | Where-Object { $_ -ne '' })
    if ($lines.Count -eq 0) { return }

    if ($lines[0] -eq 'dirs') {
        Get-ChildItem -Path "$wordToComplete*" -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_.FullName, $_.Name, 'ProviderContainer', $_.FullName)
        }
        return
    }
    if ($lines[0] -eq 'files') {
        Get-ChildItem -Path "$wordToComplete*" -ErrorAction SilentlyContinue | ForEach-Object {
            $t = if ($_.PSIsContainer) { 'ProviderContainer' } else { 'ProviderItem' }
            [System.Management.Automation.CompletionResult]::new($_.FullName, $_.Name, $t, $_.FullName)
        }
        return
    }
    if ($lines[0] -ne 'items') { return }

    foreach ($line in $lines[1..($lines.Count - 1)]) {
        $p = $line -split "\t"
        if ($p.Count -lt 4) { continue }
        # Column 4 is the item kind; mapping it back gives the popup its icon and grouping.
        $type = switch ($p[3]) {
            'command' { 'Command' }
            'param'   { 'ParameterName' }
            default   { 'ParameterValue' }
        }
        [System.Management.Automation.CompletionResult]::new($p[0], $p[1], $type, $p[2])
    }
}
"##;
