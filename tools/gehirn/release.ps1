[CmdletBinding()]
param(
    [ValidateSet("prepare", "publish")]
    [string]$Action = "prepare",

    [ValidateSet("patch", "minor", "major")]
    [string]$Bump,

    [string]$Version,

    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Push-Location $Root

try {
    function Invoke-Git {
        param([string[]]$GitArgs)
        & git @GitArgs
        if ($LASTEXITCODE -ne 0) {
            throw "git $($GitArgs -join ' ') failed with exit code $LASTEXITCODE"
        }
    }

    function Get-GitOutput {
        param([string[]]$GitArgs)
        $output = & git @GitArgs 2>$null
        if ($LASTEXITCODE -ne 0) {
            throw "git $($GitArgs -join ' ') failed with exit code $LASTEXITCODE"
        }
        return @($output)
    }

    function Invoke-Tool {
        param([string]$Exe, [string[]]$ToolArgs)
        & $Exe @ToolArgs
        if ($LASTEXITCODE -ne 0) {
            throw "$Exe $($ToolArgs -join ' ') failed with exit code $LASTEXITCODE"
        }
    }

    function Read-Text {
        param([string]$Path)
        return [System.IO.File]::ReadAllText($Path)
    }

    function Write-Text {
        param([string]$Path, [string]$Text)
        $utf8 = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllText($Path, $Text, $utf8)
    }

    function Assert-Clean {
        $status = @(Get-GitOutput @("status", "--porcelain"))
        if ($status.Count -gt 0) {
            throw "working tree is not clean; commit or stash changes first"
        }
    }

    function Get-CurrentVersion {
        $text = Read-Text $CargoPath
        if ($text -notmatch '(?m)^version = "(\d+\.\d+\.\d+)"') {
            throw "cannot read the package version from Cargo.toml"
        }
        return $Matches[1]
    }

    function Get-NextVersion {
        param([string]$Current)

        if ($Version) {
            if ($Version -notmatch '^\d+\.\d+\.\d+$') {
                throw "version must be MAJOR.MINOR.PATCH"
            }
            return $Version
        }

        $mode = $Bump
        if (-not $mode) {
            $answer = Read-Host "current version $Current; bump [p]atch, [m]inor, [M]ajor, or enter X.Y.Z"
            if ($answer -match '^\d+\.\d+\.\d+$') {
                return $answer
            }
            if ($answer -ceq "M" -or $answer -ieq "major") {
                $mode = "major"
            } elseif ($answer -ieq "m" -or $answer -ieq "minor") {
                $mode = "minor"
            } elseif ($answer -ieq "p" -or $answer -ieq "patch") {
                $mode = "patch"
            } else {
                throw "unknown bump: $answer"
            }
        }

        $parts = $Current.Split(".")
        $major = [int]$parts[0]
        $minor = [int]$parts[1]
        $patch = [int]$parts[2]
        switch ($mode) {
            "major" { return "$($major + 1).0.0" }
            "minor" { return "$major.$($minor + 1).0" }
            "patch" { return "$major.$minor.$($patch + 1)" }
        }
    }

    function Get-PreviousTag {
        $tag = & git describe --tags --abbrev=0 --match "v[0-9]*" 2>$null
        if ($LASTEXITCODE -eq 0 -and $tag) {
            return ($tag | Select-Object -First 1).Trim()
        }
        return $null
    }

    function Get-ChangeLines {
        param([string]$PreviousTag)

        $gitLogArgs = @("log", "--no-merges", "--pretty=format:- %s (%h)")
        if ($PreviousTag) {
            $gitLogArgs += "$PreviousTag..HEAD"
        }
        $lines = @(Get-GitOutput $gitLogArgs)
        if ($lines.Count -eq 0) {
            return @("- No user-facing changes.")
        }
        return $lines
    }

    $CargoPath = Join-Path $Root "Cargo.toml"
    $ChangelogPath = Join-Path $Root "CHANGELOG.md"
    $ReadmePath = Join-Path $Root "README.md"

    if (-not (Test-Path -LiteralPath $CargoPath)) {
        throw "Cargo.toml not found"
    }
    if (-not (Test-Path -LiteralPath $ChangelogPath)) {
        throw "CHANGELOG.md not found"
    }
    if (-not (Test-Path -LiteralPath $ReadmePath)) {
        throw "README.md not found"
    }

    Assert-Clean
    $current = Get-CurrentVersion

    if ($Action -eq "prepare") {
        $next = Get-NextVersion -Current $current
        if ([version]$next -le [version]$current) {
            throw "new version $next must be greater than $current"
        }

        $previousTag = Get-PreviousTag
        $changes = @(Get-ChangeLines -PreviousTag $previousTag)
        $date = Get-Date -Format "yyyy-MM-dd"
        $summary = ""
        if (-not $DryRun) {
            $summary = Read-Host "release summary (optional)"
        }

        $entry = "## [$next] - $date`n`n"
        if ($summary) {
            $entry += "$summary`n`n"
        }
        $entry += "### Changes`n`n" + ($changes -join "`n") + "`n`n"

        $changelog = Read-Text $ChangelogPath
        $marker = "`n## ["
        $index = $changelog.IndexOf($marker)
        if ($index -ge 0) {
            $changelog = $changelog.Substring(0, $index + 1) + $entry + $changelog.Substring($index + 1)
        } else {
            $changelog = $changelog.TrimEnd() + "`n`n" + $entry
        }

        $readme = Read-Text $ReadmePath
        $oldStatus = "**v$current**"
        if ($readme.IndexOf($oldStatus) -lt 0) {
            throw "README status line does not contain $oldStatus"
        }
        $readme = $readme.Replace($oldStatus, "**v$next**")

        $cargo = Read-Text $CargoPath
        $cargo = [regex]::Replace(
            $cargo,
            '(?m)^version = "' + [regex]::Escape($current) + '"',
            "version = `"$next`""
        )

        Write-Host "preparing MAGI v$next (was v$current, previous tag: $previousTag)"
        if ($changes.Count -eq 1 -and $changes[0] -eq "- No user-facing changes.") {
            Write-Host "warning: no commits since the previous tag"
        }

        if ($DryRun) {
            Write-Host "`n--- CHANGELOG entry ---"
            Write-Host $entry
            Write-Host "--- README / Cargo.toml would be updated; no commit or tag written ---"
            return
        }

        Write-Text $ChangelogPath $changelog
        Write-Text $ReadmePath $readme
        Write-Text $CargoPath $cargo

        Invoke-Tool "cargo" @("check", "--quiet")
        Invoke-Tool "cargo" @("fmt", "--check")
        Invoke-Tool "cargo" @("test", "--quiet")
        Invoke-Tool "cargo" @("run", "--quiet", "--", "--selftest")

        Invoke-Git @("add", "CHANGELOG.md", "Cargo.toml", "Cargo.lock", "README.md")
        Invoke-Git @("commit", "-m", "release v$next")
        Invoke-Git @("tag", "-a", "v$next", "-m", "MAGI v$next")

        Write-Host "`nMAGI v$next prepared and tagged locally."
        Write-Host "Next: release.cmd option G to push main and the tag."
        return
    }

    if ($Action -eq "publish") {
        $branch = (Get-GitOutput @("rev-parse", "--abbrev-ref", "HEAD") | Select-Object -First 1).Trim()
        if ($branch -ne "main") {
            throw "publish must run from main (currently $branch)"
        }
        $tag = "v$current"
        $tagCommit = (Get-GitOutput @("rev-list", "-n", "1", $tag) | Select-Object -First 1)
        if (-not $tagCommit) {
            throw "$tag does not exist; prepare the release first"
        }
        $head = (Get-GitOutput @("rev-parse", "HEAD") | Select-Object -First 1).Trim()
        if ($tagCommit.Trim() -ne $head) {
            throw "$tag is not at HEAD; check out the release commit first"
        }

        Write-Host "publishing v$current from main"
        if ($DryRun) {
            Write-Host "would run: git push --force-with-lease origin main"
            Write-Host "would run: git push --force origin $tag"
            return
        }
        # with-lease still refuses if the remote moved behind our back, and
        # covers a rewritten history (squashes, rebases).
        Invoke-Git @("push", "--force-with-lease", "origin", "main")
        Invoke-Git @("push", "--force", "origin", $tag)
        Write-Host "`nPushed main and $tag. CI will publish the GitHub release."
        return
    }
}
catch {
    Write-Host "`nrelease: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
finally {
    Pop-Location
}
