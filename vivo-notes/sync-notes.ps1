#Requires -Version 5.1
<#
.SYNOPSIS
    把 vivo 办公套件「原子笔记」同步到本机（Markdown + 原始 JSON 备份）。

.DESCRIPTION
    直接调用办公套件本机的第三方接口（默认 127.0.0.1:9200），不依赖 Skill。
    读取 %USERPROFILE%\.notes-cli.conf 中的 NOTES_PORT / NOTES_TOKEN。

    输出：
      notes\            每条笔记一个 .md，按原子笔记的文件夹分组
      assets\<id>\      笔记里引用的图片
      notes.json        全部笔记的原始数据（含 HTML 正文，完整备份）
      index.md          笔记索引
      sync-state.json   增量同步状态
      sync.log          同步日志

.PARAMETER OutDir
    输出根目录，默认是工具目录下的 data 文件夹。

.PARAMETER Full
    忽略增量状态，强制重新下载全部笔记。

.PARAMETER Prune
    云端已删除（或进了回收站）的笔记，同时删除本地对应的 .md 文件。
    默认不删除，只在日志里提示。

.PARAMETER Quiet
    不输出到控制台（仍然写 sync.log），适合计划任务。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\sync-notes.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\sync-notes.ps1 -Full -Prune
#>
[CmdletBinding()]
param(
    [string]$OutDir = '',
    [switch]$Full,
    [switch]$Prune,
    [switch]$Quiet
)

if (-not $OutDir) {
    $root = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
    $OutDir = Join-Path $root 'data'
}

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

# ---------------------------------------------------------------- 配置
$ConfigFile = Join-Path $env:USERPROFILE '.notes-cli.conf'
$script:Port  = if ($env:NOTES_PORT)  { $env:NOTES_PORT }  else { '9200' }
$script:Token = if ($env:NOTES_TOKEN) { $env:NOTES_TOKEN } else { '' }

if (Test-Path -LiteralPath $ConfigFile) {
    foreach ($line in Get-Content -LiteralPath $ConfigFile -Encoding UTF8) {
        $line = $line.TrimStart([char]0xFEFF)
        if ($line -match '^\s*([^=]+?)\s*=\s*(.*)$') {
            switch ($matches[1].Trim()) {
                'NOTES_PORT'  { $script:Port  = $matches[2].Trim() }
                'NOTES_TOKEN' { $script:Token = $matches[2].Trim() }
            }
        }
    }
}

$script:BaseUrl = "http://127.0.0.1:$script:Port/third-party"
$script:HealthUrl = "http://127.0.0.1:$script:Port/health"
$script:ResourceRoot = Join-Path $env:APPDATA 'pcsuite\data\Note\Resource'

$script:NotesDir  = Join-Path $OutDir 'notes'
$script:AssetsDir = Join-Path $OutDir 'assets'
$script:RawFile   = Join-Path $OutDir 'notes.json'
$script:StateFile = Join-Path $OutDir 'sync-state.json'
$script:IndexFile = Join-Path $OutDir 'index.md'
$script:LogFile   = Join-Path $OutDir 'sync.log'
$script:LockFile  = Join-Path $OutDir '.sync.lock'

foreach ($d in @($OutDir, $script:NotesDir, $script:AssetsDir)) {
    if (-not (Test-Path -LiteralPath $d)) {
        New-Item -ItemType Directory -Path $d -Force | Out-Null
    }
}

# ---------------------------------------------------------------- 工具函数
function Write-SyncLog {
    param([string]$Message)
    $line = '{0}  {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Message
    try {
        # 日志超过 2MB 时轮转一次
        if ((Test-Path -LiteralPath $script:LogFile) -and (Get-Item -LiteralPath $script:LogFile).Length -gt 2MB) {
            Move-Item -LiteralPath $script:LogFile -Destination "$($script:LogFile).1" -Force
        }
        [System.IO.File]::AppendAllText($script:LogFile, $line + "`r`n", (New-Object System.Text.UTF8Encoding($false)))
    } catch { }
    if (-not $Quiet) { Write-Host $Message }
}

function Write-Utf8NoBom {
    param([string]$Path, [string]$Text)
    $dir = Split-Path -Parent $Path
    if ($dir -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding($false)))
}

function Invoke-NotesApi {
    param([string]$Path)
    $headers = @{}
    if ($script:Token) { $headers['Authorization'] = "Bearer $($script:Token)" }
    $resp = Invoke-WebRequest -Uri "$($script:BaseUrl)$Path" -Headers $headers -UseBasicParsing -TimeoutSec 60
    $raw = $resp.RawContentStream
    if ($null -ne $raw) {
        $text = [System.Text.Encoding]::UTF8.GetString($raw.ToArray())
    } else {
        $text = [string]$resp.Content
    }
    return ($text | ConvertFrom-Json)
}

function Test-NotesService {
    try {
        $resp = Invoke-WebRequest -Uri $script:HealthUrl -UseBasicParsing -TimeoutSec 10
        return ($resp.StatusCode -eq 200)
    } catch {
        return $false
    }
}

function Get-AllNotes {
    $all = New-Object System.Collections.ArrayList
    $page = 1
    while ($true) {
        $r = Invoke-NotesApi "/notes?limit=100&page=$page"
        if ($r.status -ne 'success') { throw "获取笔记列表失败：$($r | ConvertTo-Json -Compress -Depth 5)" }
        foreach ($n in @($r.data)) { [void]$all.Add($n) }
        $total = [int]$r.meta.total
        if ($all.Count -ge $total -or @($r.data).Count -eq 0) { break }
        $page++
    }
    return $all.ToArray()
}

function Get-FolderMap {
    $map = @{}
    $visited = @{}
    function Walk([string]$ParentId, [string]$Prefix) {
        $url = if ($ParentId) { "/folders?parent=$([uri]::EscapeDataString($ParentId))" } else { '/folders' }
        try { $r = Invoke-NotesApi $url } catch { return }
        foreach ($f in @($r.data)) {
            if (-not $f -or -not $f.id -or $visited.ContainsKey($f.id)) { continue }
            $visited[$f.id] = $true
            $name = [string]$f.name
            $full = if ($Prefix) { "$Prefix/$name" } else { $name }
            $map[[string]$f.id] = $full
            Walk ([string]$f.id) $full
        }
    }
    Walk '' ''
    return $map
}

function Get-SafeName {
    param([string]$Name, [int]$MaxLength = 70)
    if ([string]::IsNullOrWhiteSpace($Name)) { $Name = '无标题' }
    $invalid = [System.IO.Path]::GetInvalidFileNameChars()
    foreach ($c in $invalid) { $Name = $Name.Replace($c, '_') }
    $Name = $Name -replace '[\x00-\x1F]', ''
    $Name = ($Name -replace '\s+', ' ').Trim().TrimEnd('.')
    if ($Name.Length -gt $MaxLength) { $Name = $Name.Substring(0, $MaxLength).TrimEnd() }
    if ([string]::IsNullOrWhiteSpace($Name)) { $Name = '无标题' }
    $reserved = '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$'
    if ($Name -match $reserved) { $Name = '_' + $Name }
    return $Name
}

function Get-NoteDisplayTitle {
    param($Note)
    $t = [string]$Note.title
    if (-not [string]::IsNullOrWhiteSpace($t)) { return $t.Trim() }
    $plain = [string]$Note.content
    $plain = [regex]::Replace($plain, '(?is)<(script|style)[^>]*>.*?</\1>', '')
    $plain = [regex]::Replace($plain, '(?s)<[^>]+>', "`n")
    $plain = [System.Net.WebUtility]::HtmlDecode($plain)
    foreach ($line in ($plain -split "`n")) {
        $line = ($line -replace '^[\s#>\-\*]+', '').Trim()
        if ($line) {
            if ($line.Length -gt 30) { $line = $line.Substring(0, 30) }
            return $line
        }
    }
    return '无标题'
}

function ConvertTo-YamlString {
    param([string]$Value)
    if ($null -eq $Value) { $Value = '' }
    $escaped = $Value.Replace('\', '\\').Replace('"', '\"').Replace("`r", '').Replace("`n", '\n')
    return '"' + $escaped + '"'
}

function Get-RelativePath {
    param([string]$FromDirectory, [string]$ToPath)
    $from = $FromDirectory.TrimEnd('\') + '\'
    $fromUri = New-Object System.Uri($from)
    $toUri = New-Object System.Uri($ToPath)
    $rel = $fromUri.MakeRelativeUri($toUri).ToString()
    return [System.Uri]::UnescapeDataString($rel)
}

function Copy-NoteImage {
    param(
        [string]$NoteId,
        [string]$Source,       # 文件名 / 本地路径 / http(s) URL
        [string]$NoteDirectory
    )
    if ([string]::IsNullOrWhiteSpace($Source)) { return $null }

    $safe = Get-SafeName -Name ([System.IO.Path]::GetFileName(($Source -split '\?')[0])) -MaxLength 180
    $destDir = Join-Path $script:AssetsDir $NoteId
    if (-not (Test-Path -LiteralPath $destDir)) {
        New-Item -ItemType Directory -Path $destDir -Force | Out-Null
    }
    $dest = Join-Path $destDir $safe

    $copied = $false
    if ($Source -match '^https?://') {
        if (-not (Test-Path -LiteralPath $dest)) {
            try {
                Invoke-WebRequest -Uri $Source -OutFile $dest -UseBasicParsing -TimeoutSec 60
                $copied = $true
            } catch {
                return $null
            }
        } else { $copied = $true }
    } elseif (Test-Path -LiteralPath $Source -PathType Leaf) {
        if (-not (Test-Path -LiteralPath $dest)) {
            Copy-Item -LiteralPath $Source -Destination $dest -Force
            $copied = $true
        } else { $copied = $true }
    } else {
        # 办公套件把笔记图片缓存在 data\Note\Resource 下，按文件名查找
        if (Test-Path -LiteralPath $script:ResourceRoot) {
            $found = Get-ChildItem -LiteralPath $script:ResourceRoot -Recurse -File -Filter $safe -ErrorAction SilentlyContinue |
                     Select-Object -First 1
            if ($found) {
                if (-not (Test-Path -LiteralPath $dest)) {
                    Copy-Item -LiteralPath $found.FullName -Destination $dest -Force
                }
                $copied = $true
            }
        }
    }

    if (-not $copied) { return $null }
    $rel = Get-RelativePath -FromDirectory $NoteDirectory -ToPath $dest
    return $rel.Replace('\', '/')
}

function Convert-HtmlToMarkdown {
    param([string]$Html)
    if ([string]::IsNullOrWhiteSpace($Html)) { return '' }

    $s = $Html -replace "`r`n", "`n" -replace "`r", "`n"
    $s = [regex]::Replace($s, '(?is)<(script|style)[^>]*>.*?</\1>', '')

    # 自定义图片标签 -> 占位符
    $s = [regex]::Replace($s, '(?is)<vnote-image[^>]*\bfilename\s*=\s*["'']([^"'']+)["''][^>]*>', {
        param($m)
        "`n{{IMG:$($m.Groups[1].Value)}}`n"
    })
    $s = [regex]::Replace($s, '(?is)<img([^>]*)>', {
        param($m)
        $attrs = $m.Groups[1].Value
        $src = ''; $alt = ''
        if ($attrs -match 'src\s*=\s*["'']([^"'']+)["'']') { $src = $matches[1] }
        if ($attrs -match 'alt\s*=\s*["'']([^"'']*)["'']') { $alt = $matches[1] }
        if ($src) { "`n{{IMG:$src|$alt}}`n" } else { '' }
    })

    # 标题
    $s = [regex]::Replace($s, '(?is)<h([1-6])[^>]*>(.*?)</h\1>', {
        param($m)
        $level = [int]$m.Groups[1].Value
        "`n" + ('#' * $level) + ' ' + $m.Groups[2].Value.Trim() + "`n"
    })

    # 块级元素
    $s = [regex]::Replace($s, '(?i)<br\s*/?>', "`n")
    $s = [regex]::Replace($s, '(?i)</p\s*>', "`n`n")
    $s = [regex]::Replace($s, '(?i)<p[^>]*>', '')
    $s = [regex]::Replace($s, '(?i)</div\s*>', "`n")
    $s = [regex]::Replace($s, '(?i)<div[^>]*>', '')
    $s = [regex]::Replace($s, '(?i)<hr[^>]*>', "`n---`n")

    # 列表
    $s = [regex]::Replace($s, '(?is)<li[^>]*>(.*?)</li>', {
        param($m) "`n- " + $m.Groups[1].Value.Trim()
    })
    $s = [regex]::Replace($s, '(?i)</?[ou]l[^>]*>', "`n")

    # 行内样式
    $s = [regex]::Replace($s, '(?is)<(strong|b)[^>]*>(.*?)</\1>', { param($m) '**' + $m.Groups[2].Value + '**' })
    $s = [regex]::Replace($s, '(?is)<(em|i)[^>]*>(.*?)</\1>',     { param($m) '*'  + $m.Groups[2].Value + '*' })
    $s = [regex]::Replace($s, '(?is)<(del|s|strike)[^>]*>(.*?)</\1>', { param($m) '~~' + $m.Groups[2].Value + '~~' })
    $s = [regex]::Replace($s, '(?is)<code[^>]*>(.*?)</code>',      { param($m) '`' + $m.Groups[1].Value + '`' })
    $s = [regex]::Replace($s, '(?is)<blockquote[^>]*>(.*?)</blockquote>', {
        param($m) "`n> " + ($m.Groups[1].Value -replace "`n", "`n> ") + "`n"
    })
    $s = [regex]::Replace($s, '(?is)<a[^>]*href\s*=\s*["'']([^"'']+)["''][^>]*>(.*?)</a>', {
        param($m) '[' + $m.Groups[2].Value + '](' + $m.Groups[1].Value + ')'
    })

    # 去掉剩余标签
    $s = [regex]::Replace($s, '(?s)<[^>]+>', '')
    $s = [System.Net.WebUtility]::HtmlDecode($s)
    $s = $s.Replace([char]0x00A0, ' ')
    $s = [regex]::Replace($s, '[ \t]+\n', "`n")
    $s = [regex]::Replace($s, "\n{3,}", "`n`n")
    return $s.Trim()
}

function Format-NoteMarkdown {
    param($Note, [string]$FolderPath, [string]$NoteDirectory, [string]$DisplayTitle)
    $content = [string]$Note.content

    if ($content -match '<[a-zA-Z][^>]*>') {
        $body = Convert-HtmlToMarkdown -Html $content
    } else {
        $body = $content.Trim()
    }

    # 处理正文里的图片占位符
    $body = [regex]::Replace($body, '\{\{IMG:([^}]+)\}\}', {
        param($m)
        $payload = $m.Groups[1].Value
        $parts = $payload -split '\|', 2
        $src = $parts[0]
        $alt = if ($parts.Count -gt 1) { $parts[1] } else { '' }
        $rel = Copy-NoteImage -NoteId ([string]$Note.id) -Source $src -NoteDirectory $NoteDirectory
        if ($rel) {
            '![{0}](<{1}>)' -f $alt, $rel
        } else {
            '[图片：{0}]' -f $src
        }
    })

    $title = if ([string]::IsNullOrWhiteSpace($DisplayTitle)) { '无标题' } else { $DisplayTitle }
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine('---')
    [void]$sb.AppendLine('id: ' + (ConvertTo-YamlString ([string]$Note.id)))
    [void]$sb.AppendLine('title: ' + (ConvertTo-YamlString ([string]$Note.title)))
    [void]$sb.AppendLine('folder: ' + (ConvertTo-YamlString $FolderPath))
    [void]$sb.AppendLine('created: ' + (ConvertTo-YamlString ([string]$Note.created)))
    [void]$sb.AppendLine('updated: ' + (ConvertTo-YamlString ([string]$Note.updated)))
    [void]$sb.AppendLine('---')
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine('# ' + $title)
    [void]$sb.AppendLine('')
    if ($body) { [void]$sb.AppendLine($body) }
    return $sb.ToString().TrimEnd() + "`n"
}

# ---------------------------------------------------------------- 同步

# 全屏游戏运行时直接跳过：抢焦点会害人，笔记晚几分钟同步没有关系
$gameProcesses = @('r5apex', 'r5apex_dx12')
$runningGame = Get-Process -Name $gameProcesses -ErrorAction SilentlyContinue
if ($runningGame) {
    Write-SyncLog "检测到游戏正在运行（$($runningGame[0].ProcessName)），本次跳过同步。"
    exit 0
}

try {
    [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = [System.Diagnostics.ProcessPriorityClass]::BelowNormal
} catch { }

$lock = $null
try {
    $lock = [System.IO.File]::Open($script:LockFile, 'OpenOrCreate', 'ReadWrite', 'None')
} catch {
    if (-not $Quiet) { Write-Host '另一个同步正在进行，本次跳过。' }
    exit 0
}

try {
    Write-SyncLog '开始同步原子笔记...'

    if (-not (Test-NotesService)) {
        Write-SyncLog "无法连接办公套件（$($script:HealthUrl)）。请确认 vivo 办公套件正在运行。"
        exit 2
    }
    if (-not $script:Token) {
        Write-SyncLog "没有找到 Token，请先运行：notes config --token=<token>"
        exit 3
    }

    $remoteNotes = @(Get-AllNotes)
    Write-SyncLog "云端共 $($remoteNotes.Count) 条笔记，正在检查本地..."

    $folderMap = Get-FolderMap

    # 读取增量状态
    $stateNotes = @{}
    $lastSync = $null
    if (Test-Path -LiteralPath $script:StateFile) {
        try {
            $stateRaw = Get-Content -LiteralPath $script:StateFile -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($stateRaw.lastSync) { $lastSync = $stateRaw.lastSync }
            if ($stateRaw.notes) {
                foreach ($p in $stateRaw.notes.PSObject.Properties) { $stateNotes[$p.Name] = $p.Value }
            }
        } catch {
            Write-SyncLog "状态文件损坏，将重新同步全部笔记。"
        }
    }

    # 旧的原始备份（用于跳过未变化笔记时复用）
    $oldRaw = @{}
    if (Test-Path -LiteralPath $script:RawFile) {
        try {
            $rawJson = Get-Content -LiteralPath $script:RawFile -Raw -Encoding UTF8 | ConvertFrom-Json
            foreach ($x in @($rawJson.notes)) { if ($x.id) { $oldRaw[[string]$x.id] = $x } }
        } catch { }
    }

    $newStateNotes = @{}
    $rawOut = New-Object System.Collections.ArrayList
    $added = 0; $updated = 0; $skipped = 0; $failed = 0; $pruned = 0
    $errors = New-Object System.Collections.ArrayList
    $remoteIds = @{}

    foreach ($n in $remoteNotes) {
        $id = [string]$n.id
        $remoteIds[$id] = $true

        $folderId = [string]$n.folder
        $folderPath = if ($folderId -and $folderId -ne '0' -and $folderMap.ContainsKey($folderId)) { [string]$folderMap[$folderId] } else { '' }

        $segments = @()
        if ($folderPath) {
            foreach ($seg in ($folderPath -split '/')) {
                if ($seg) { $segments += (Get-SafeName -Name $seg -MaxLength 50) }
            }
        }
        $noteDir = if ($segments.Count -gt 0) { Join-Path $script:NotesDir ($segments -join '\') } else { $script:NotesDir }
        $fileName = (Get-SafeName -Name ([string]$n.title) -MaxLength 70) + ' [' + $id.Substring(0, [Math]::Min(8, $id.Length)) + '].md'
        $relPath = if ($segments.Count -gt 0) { 'notes\' + ($segments -join '\') + '\' + $fileName } else { 'notes\' + $fileName }
        $fullPath = Join-Path $OutDir $relPath

        $prev = $stateNotes[$id]
        $prevPathExists = $prev -and $prev.path -and (Test-Path -LiteralPath (Join-Path $OutDir ([string]$prev.path)))
        $unchanged = (-not $Full) -and $prev -and
                     ([string]$prev.updated -eq [string]$n.updated) -and
                     ([string]$prev.title -eq [string]$n.title) -and
                     ([string]$prev.folder -eq $folderId) -and
                     $prevPathExists

        if ($unchanged) {
            $newStateNotes[$id] = $prev
            $skipped++
            if ($oldRaw.ContainsKey($id)) { [void]$rawOut.Add($oldRaw[$id]) }
            continue
        }

        try {
            $detail = Invoke-NotesApi "/notes/$id"
            if ($detail.status -ne 'success') { throw "接口返回失败：$($detail | ConvertTo-Json -Compress -Depth 5)" }
            $note = $detail.data

            # 标题以详情为准；没有标题时用正文首行生成可读文件名
            $displayTitle = Get-NoteDisplayTitle -Note $note
            $fileName = (Get-SafeName -Name $displayTitle -MaxLength 70) + ' [' + $id.Substring(0, [Math]::Min(8, $id.Length)) + '].md'
            $relPath = if ($segments.Count -gt 0) { 'notes\' + ($segments -join '\') + '\' + $fileName } else { 'notes\' + $fileName }
            $fullPath = Join-Path $OutDir $relPath
            $noteDir = Split-Path -Parent $fullPath

            $md = Format-NoteMarkdown -Note $note -FolderPath $folderPath -NoteDirectory $noteDir -DisplayTitle $displayTitle
            Write-Utf8NoBom -Path $fullPath -Text $md

            # 笔记移动/改名后删除旧文件
            if ($prev -and $prev.path -and ([string]$prev.path -ne $relPath)) {
                $oldPath = Join-Path $OutDir ([string]$prev.path)
                if (Test-Path -LiteralPath $oldPath) {
                    Remove-Item -LiteralPath $oldPath -Force -ErrorAction SilentlyContinue
                }
            }

            $newStateNotes[$id] = [pscustomobject]@{
                updated = [string]$n.updated
                title   = [string]$n.title
                folder  = $folderId
                path    = $relPath
            }
            [void]$rawOut.Add($note)

            if ($prev) { $updated++ } else { $added++ }
        } catch {
            $failed++
            [void]$errors.Add("$id [$($n.title)]: $($_.Exception.Message)")
            if ($prev) { $newStateNotes[$id] = $prev }
            if ($oldRaw.ContainsKey($id)) { [void]$rawOut.Add($oldRaw[$id]) }
        }
    }

    # 云端删除的笔记
    $removedIds = @($stateNotes.Keys | Where-Object { -not $remoteIds.ContainsKey($_) })
    foreach ($id in $removedIds) {
        $prev = $stateNotes[$id]
        if ($prev -and $prev.path) {
            $p = Join-Path $OutDir ([string]$prev.path)
            if (Test-Path -LiteralPath $p) {
                if ($Prune) {
                    Remove-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
                    $pruned++
                } else {
                    Write-SyncLog "笔记已不在云端（本地文件保留）：$($prev.title)  $($prev.path)"
                }
            }
        }
    }

    # 写原始备份
    $rawDoc = [pscustomobject]@{
        syncedAt = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        count    = $rawOut.Count
        notes    = @($rawOut)
    }
    Write-Utf8NoBom -Path $script:RawFile -Text (ConvertTo-Json -InputObject $rawDoc -Depth 20)

    # 写状态
    $stateDoc = [pscustomobject]@{
        version  = 1
        lastSync = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        notes    = $newStateNotes
    }
    Write-Utf8NoBom -Path $script:StateFile -Text (ConvertTo-Json -InputObject $stateDoc -Depth 10)

    # 写索引
    $indexRows = foreach ($n in ($rawOut | Sort-Object @{ Expression = { [string]$_.updated }; Descending = $true })) {
        $safeTitle = Get-NoteDisplayTitle -Note $n
        $safeTitle = $safeTitle.Replace('|', '\|')
        $fid = [string]$n.folder
        $fpath = if ($fid -and $fid -ne '0' -and $folderMap.ContainsKey($fid)) { [string]$folderMap[$fid] } else { '未分类' }
        $fpath = $fpath.Replace('|', '\|')
        $st = $newStateNotes[[string]$n.id]
        $link = if ($st -and $st.path) { ([string]$st.path).Replace('\', '/') } else { '' }
        "| $safeTitle | $fpath | $([string]$n.updated) | [打开](<$link>) |"
    }

    $indexMd = @(
        '# vivo 原子笔记',
        '',
        "- 最后同步：$((Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))",
        "- 笔记总数：$($rawOut.Count)",
        "- 本地目录：``notes/``",
        '',
        '| 标题 | 文件夹 | 更新时间 | 文件 |',
        '| --- | --- | --- | --- |'
    ) + @($indexRows) + @('')
    Write-Utf8NoBom -Path $script:IndexFile -Text ($indexMd -join "`r`n")

    foreach ($e in $errors) { Write-SyncLog "失败：$e" }
    Write-SyncLog "同步完成：新增 $added，更新 $updated，跳过 $skipped，删除 $pruned，失败 $failed（共 $($remoteNotes.Count) 条）"
    exit $(if ($failed -gt 0) { 1 } else { 0 })
}
catch {
    Write-SyncLog "同步出错：$($_.Exception.Message)"
    exit 1
}
finally {
    if ($lock) { $lock.Close(); $lock.Dispose() }
}
