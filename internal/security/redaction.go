package security

import (
	"regexp"
	"strings"
)

var (
	secretKeyExpr        = `(?:password|passwd|secret|api[_-]?key|[a-z0-9._-]*token[a-z0-9._-]*)`
	kvSecretPattern      = regexp.MustCompile(`(?i)(` + secretKeyExpr + `)\s*[:=]\s*(?:"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^\s"']+)`)
	kvLooseSecretPattern = regexp.MustCompile(`(?i)\b(client_secret|private_key|aws_access_key_id|aws_secret_access_key)\b\s+(?:"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^\s"']+)`)
	jsonSecretPattern    = regexp.MustCompile(`(?i)("` + secretKeyExpr + `"\s*:\s*)"(?:[^"\\]|\\.)*"`)
	authorizationPattern = regexp.MustCompile(`(?i)(authorization\s*:\s*)[^\r\n]+`)
	bearerTokenPattern   = regexp.MustCompile(`(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+`)
	pemBlockPattern      = regexp.MustCompile(`(?s)-----BEGIN [^-]+ PRIVATE KEY-----.*?-----END [^-]+ PRIVATE KEY-----`)
	cookiePattern        = regexp.MustCompile(`(?i)(cookie\s*:\s*)[^\r\n]+`)
	sshUserPattern       = regexp.MustCompile(`(?i)(ssh://)[^\s/@]+@`)
	secretLikePattern    = regexp.MustCompile(`(?i)(-----BEGIN [^-]+ PRIVATE KEY-----|` + secretKeyExpr + `|client_secret|private_key|aws_access_key_id|aws_secret_access_key|authorization|bearer\s+[A-Za-z0-9._~+/=-]+|cookie\s*:|sessionid=)`)
)

func RedactPayload(input string) string {
	if input == "" {
		return ""
	}
	out := pemBlockPattern.ReplaceAllString(input, "[REDACTED_PRIVATE_KEY]")
	out = jsonSecretPattern.ReplaceAllString(out, `${1}"[REDACTED]"`)
	out = kvSecretPattern.ReplaceAllStringFunc(out, func(match string) string {
		idx := strings.IndexAny(match, ":=")
		if idx < 0 {
			return "[REDACTED]"
		}
		return match[:idx+1] + " [REDACTED]"
	})
	out = kvLooseSecretPattern.ReplaceAllStringFunc(out, func(match string) string {
		idx := strings.IndexAny(match, " \t")
		if idx < 0 {
			return "[REDACTED]"
		}
		return match[:idx] + " [REDACTED]"
	})
	out = authorizationPattern.ReplaceAllString(out, `${1}[REDACTED]`)
	out = bearerTokenPattern.ReplaceAllString(out, "Bearer [REDACTED]")
	out = cookiePattern.ReplaceAllString(out, `${1}[REDACTED]`)
	out = sshUserPattern.ReplaceAllString(out, `${1}[REDACTED]@`)
	return out
}

func RedactForStorage(input string) string {
	trimmed := strings.TrimSpace(input)
	if trimmed == "" {
		return ""
	}
	redacted := RedactPayload(trimmed)
	if redacted == "" {
		return ""
	}
	if redacted == trimmed {
		// Fail closed: keep payload only when a redaction transform was applied.
		return ""
	}
	if secretLikePattern.MatchString(trimmed) && !strings.Contains(redacted, "[REDACTED]") {
		return ""
	}
	return redacted
}
