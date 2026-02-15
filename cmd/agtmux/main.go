package main

import (
	"context"
	"os"

	"github.com/g960059/agtmux/internal/cli"
	"github.com/g960059/agtmux/internal/config"
)

func main() {
	cfg := config.DefaultConfig()
	r := cli.NewRunner(cfg.SocketPath, os.Stdout, os.Stderr)
	os.Exit(r.Run(context.Background(), os.Args[1:]))
}
