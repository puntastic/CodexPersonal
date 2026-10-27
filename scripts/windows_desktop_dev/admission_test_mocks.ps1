# Only imported by legacy mechanical transaction/staging tests. These fake PE
# packages cannot execute. Admission itself is tested in its focused suites;
# never dot-source this file from a production operator.
function Assert-CodexDevSelectionAdmission {
    param($Candidate, $PythonPath)
    return [pscustomobject]@{ Compatible = $true; Status = 'synthetic_test_double' }
}
function Assert-CodexDevRecoveryAdmission { param($ConfigPath, $DeploymentRoot, $PythonPath) }
function Assert-CodexDevCompatibilityBinding { param($Check, $PackageDirectory) }
