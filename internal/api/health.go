package api

import "time"

type HealthResponse struct {
	SchemaVersion string    `json:"schema_version"`
	GeneratedAt   time.Time `json:"generated_at"`
	Status        string    `json:"status"`
}
