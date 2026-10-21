$ErrorActionPreference = 'Stop'
$headers = @{'User-Agent'='Veya'; 'Accept'='application/vnd.github+json'}

# Get latest run
$runs = Invoke-RestMethod -Uri 'https://api.github.com/repos/liuzifi/Veya/actions/runs?per_page=3' -Headers $headers
$run = $runs.workflow_runs | Select-Object -First 1
Write-Output "=== Latest Run ==="
Write-Output "Run ID: $($run.id)"
Write-Output "Name: $($run.name)"
Write-Output "Status: $($run.status)"
Write-Output "Conclusion: $($run.conclusion)"
Write-Output "Head SHA: $($run.head_sha)"
Write-Output ""

# Get jobs
$jobs = Invoke-RestMethod -Uri "https://api.github.com/repos/liuzifi/Veya/actions/runs/$($run.id)/jobs" -Headers $headers
foreach ($j in $jobs.jobs) {
    Write-Output "--- Job: $($j.name) | $($j.status) | $($j.conclusion) ---"
    foreach ($s in $j.steps) {
        $mark = if ($s.conclusion -eq 'failure') { ' <<< FAILED' } else { '' }
        Write-Output "  Step: $($s.name) | $($s.status) | $($s.conclusion)$mark"
    }
    Write-Output ""
}

# Try to get logs for failed job
$failedJob = $jobs.jobs | Where-Object { $_.conclusion -eq 'failure' } | Select-Object -First 1
if ($failedJob) {
    Write-Output "=== Attempting to get logs for failed job: $($failedJob.id) ==="
    try {
        $logResponse = Invoke-WebRequest -Uri "https://api.github.com/repos/liuzifi/Veya/actions/jobs/$($failedJob.id)/logs" -Headers $headers -ErrorAction Stop
        Write-Output $logResponse.Content
    } catch {
        Write-Output "Log download failed: $($_.Exception.Message)"
        # Try to get the log via the run
        Write-Output "Trying run logs..."
        try {
            $logResponse = Invoke-WebRequest -Uri "https://api.github.com/repos/liuzifi/Veya/actions/runs/$($run.id)/logs" -Headers $headers -ErrorAction Stop
            Write-Output $logResponse.Content
        } catch {
            Write-Output "Run log download also failed: $($_.Exception.Message)"
        }
    }
}
