<#
.SYNOPSIS
  manga-viewer を起動してキーを送り、アプリ自身の描画結果を撮って 1 枚に並べ、Orca で開く。

.EXAMPLE
  # 見開きにして撮る → 1 組送って撮る
  .\capture.ps1 -Book <zipかフォルダ> -Keys f,shot,'{RIGHT}',shot -Name spread
#>
param(
    # 開く冊（zip かフォルダ）。省略すると 5 頁のサンプル冊を作って使う
    [string]$Book,
    # SendKeys の文字列を順に送る。`shot` は撮影（F12）、`mouse:<x>,<y>` は窓の中身の割合位置（0〜1）へカーソルを動かす
    [string[]]$Keys = @('shot'),
    # target/screenshots/<Name>.png に保存する
    [string]$Name = 'capture',
    # 起動後、最初のキーを送るまでの待ち（ms）
    [int]$WaitMs = 3000,
    # キーとキーの間の待ち（ms）
    [int]$StepMs = 800,
    # Orca で開かない
    [switch]$NoOpen
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..\..\..").Path
$work = Join-Path $env:TEMP "mv-capture-$PID"
$shots = Join-Path $work 'shots'
New-Item -ItemType Directory -Force $shots | Out-Null
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices;
public class MvCapWin {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public struct RECT { public int L, T, R, B; }
  public struct POINT { public int X, Y; }
}
'@

if (-not $Book) {
    # 表紙 + 色違いの 4 頁（頁番号入り）
    $Book = Join-Path $work 'sample'
    New-Item -ItemType Directory -Force $Book | Out-Null
    $colors = @('#3c3c3c', '#dc5050', '#508cdc', '#5abe78', '#c8a03c')
    $font = New-Object System.Drawing.Font('Arial', 120)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'
    for ($i = 0; $i -lt $colors.Count; $i++) {
        $bmp = New-Object System.Drawing.Bitmap 700, 1000
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.Clear([System.Drawing.ColorTranslator]::FromHtml($colors[$i]))
        $g.DrawRectangle((New-Object System.Drawing.Pen([System.Drawing.Color]::White, 6)), 3, 3, 693, 993)
        $label = if ($i -eq 0) { 'COVER' } else { "$($i + 1)" }
        $g.DrawString($label, $font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF(0, 0, 700, 1000)), $fmt)
        $bmp.Save((Join-Path $Book ('{0:000}.png' -f ($i + 1))), [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
    }
}

Push-Location $repo
try { cargo build -q } finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw 'cargo build に失敗' }

$env:MV_SCREENSHOT = $shots
# 前回の窓の大きさ・見開きを持ち越さない（毎回同じ条件で撮る、#13）
$env:MV_PERSIST = '0'
$exe = Join-Path $repo 'target\debug\manga-viewer.exe'
$err = Join-Path $work 'stderr.txt'
$p = Start-Process -FilePath $exe -ArgumentList @('view', '--path', "`"$Book`"") -PassThru -RedirectStandardError $err
try {
    Start-Sleep -Milliseconds $WaitMs
    $p.Refresh()
    [MvCapWin]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
    Start-Sleep -Milliseconds 500
    # 物理ピクセルで座標を扱う（150% 表示などでカーソル位置がずれないように）
    [MvCapWin]::SetProcessDPIAware() | Out-Null
    $hwnd = $p.MainWindowHandle
    foreach ($k in $Keys) {
        if ($k -match '^mouse:([\d.]+),([\d.]+)$') {
            $r = New-Object MvCapWin+RECT
            [MvCapWin]::GetClientRect($hwnd, [ref]$r) | Out-Null
            $pt = New-Object MvCapWin+POINT
            $pt.X = [int]($r.R * [double]$Matches[1]); $pt.Y = [int]($r.B * [double]$Matches[2])
            [MvCapWin]::ClientToScreen($hwnd, [ref]$pt) | Out-Null
            # 窓の外から一気に移すと egui がホバーを拾わないことがあるので、すぐ手前を経由する
            [MvCapWin]::SetCursorPos($pt.X - 4, $pt.Y - 4) | Out-Null
            Start-Sleep -Milliseconds 150
            [MvCapWin]::SetCursorPos($pt.X, $pt.Y) | Out-Null
        } else {
            $send = if ($k -eq 'shot') { '{F12}' } else { $k }
            [System.Windows.Forms.SendKeys]::SendWait($send)
        }
        Start-Sleep -Milliseconds $StepMs
    }
    Start-Sleep -Milliseconds 1000
} finally {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

$files = @(Get-ChildItem $shots -Filter 'shot-*.png' | Sort-Object Name)
if ($files.Count -eq 0) { throw "撮れていない（$err を確認）" }

# 撮った順に左から並べて 1 枚にする（長辺 1600 まで縮める）
$imgs = $files | ForEach-Object { [System.Drawing.Image]::FromFile($_.FullName) }
$gap = 40
$w = ($imgs | Measure-Object Width -Sum).Sum + $gap * ($imgs.Count - 1)
$h = ($imgs | Measure-Object Height -Maximum).Maximum
$scale = [Math]::Min(1.0, 1600.0 / [Math]::Max($w, $h))
$out = New-Object System.Drawing.Bitmap ([int]($w * $scale)), ([int]($h * $scale))
$g = [System.Drawing.Graphics]::FromImage($out)
$g.Clear([System.Drawing.Color]::Black)
$g.InterpolationMode = 'HighQualityBicubic'
$x = 0
foreach ($im in $imgs) {
    $g.DrawImage($im, [int]($x * $scale), 0, [int]($im.Width * $scale), [int]($im.Height * $scale))
    $x += $im.Width + $gap
}
$dest = Join-Path $repo "target\screenshots\$Name.png"
New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
$out.Save($dest, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $out.Dispose(); $imgs | ForEach-Object { $_.Dispose() }
Remove-Item -Recurse -Force $work

"saved`ttarget\screenshots\$Name.png`t($($files.Count) shots)"
if (-not $NoOpen) {
    Push-Location $repo
    try { orca file open "target/screenshots/$Name.png" --json | Out-Null } finally { Pop-Location }
    "opened in Orca"
}
