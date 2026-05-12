# 批量修复 jcode-rag 文件中的 Unicode 字符问题
$files = @(
    "d:\studying\Codecargo\opensource\jcode\crates\jcode-rag\src\debugging_layer.rs",
    "d:\studying\Codecargo\opensource\jcode\crates\jcode-rag\src\editing_layer.rs",
    "d:\studying\Codecargo\opensource\jcode\crates\jcode-rag\src\indexing_layer.rs",
    "d:\studying\Codecargo\opensource\jcode\crates\jcode-rag\src\retrieval_layer.rs",
    "d:\studying\Codecargo\opensource\jcode\crates\jcode-rag\src\validation_layer.rs"
)

foreach ($file in $files) {
    if (Test-Path $file) {
        Write-Host "Processing: $file"
        $content = Get-Content $file -Raw -Encoding UTF8
        
        # 移除文档注释中的 box-drawing 字符块 (简化处理)
        # 匹配 ``` 到 ``` 之间的内容，并移除其中的特殊字符
        $content = $content -replace '[┌┐└┘├┤┬┴┼─│]', ' '
        
        Set-Content $file -Value $content -Encoding UTF8 -NoNewline
        Write-Host "  Fixed: $file"
    }
}

Write-Host "`nDone! All files processed."
