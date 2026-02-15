package config

import (
	"os"
	"path/filepath"
	"time"
)

type Config struct {
	SocketPath              string
	DBPath                  string
	SnapshotTTL             time.Duration
	CompletedDemotionAfter  time.Duration
	StaleSignalTTL          time.Duration
	BindWindow              time.Duration
	PendingBindTTL          time.Duration
	SkewBudget              time.Duration
	ActiveReconcileInterval time.Duration
	IdleReconcileInterval   time.Duration
	ConnectTimeout          time.Duration
	CommandTimeout          time.Duration
	RetryBackoff            []time.Duration
	TargetDownWindow        time.Duration
	TargetDownFailures      int
	TargetRecoverSuccesses  int
	EventPayloadTTL         time.Duration
	EventMetadataTTL        time.Duration
}

func DefaultConfig() Config {
	return Config{
		SocketPath:              defaultSocketPath(),
		DBPath:                  defaultDBPath(),
		SnapshotTTL:             30 * time.Second,
		CompletedDemotionAfter:  120 * time.Second,
		StaleSignalTTL:          30 * time.Second,
		BindWindow:              5 * time.Second,
		PendingBindTTL:          30 * time.Second,
		SkewBudget:              10 * time.Second,
		ActiveReconcileInterval: 2 * time.Second,
		IdleReconcileInterval:   10 * time.Second,
		ConnectTimeout:          3 * time.Second,
		CommandTimeout:          5 * time.Second,
		RetryBackoff:            []time.Duration{250 * time.Millisecond, 1 * time.Second},
		TargetDownWindow:        30 * time.Second,
		TargetDownFailures:      3,
		TargetRecoverSuccesses:  2,
		EventPayloadTTL:         7 * 24 * time.Hour,
		EventMetadataTTL:        14 * 24 * time.Hour,
	}
}

func defaultSocketPath() string {
	runtimeDir := os.Getenv("XDG_RUNTIME_DIR")
	if runtimeDir != "" {
		return filepath.Join(runtimeDir, "agtmux", "agtmuxd.sock")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return ".agtmuxd.sock"
	}
	return filepath.Join(home, ".local", "state", "agtmux", "agtmuxd.sock")
}

func defaultDBPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return "agtmux.db"
	}
	return filepath.Join(home, ".local", "state", "agtmux", "state.db")
}
