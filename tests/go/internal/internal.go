package internal

import (
	"log/slog"
	"os"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats-server/v2/test"
)

func init() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		AddSource: true,
		Level:     slog.LevelDebug,
		ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

func RunNats(t *testing.T) *server.Server {
	opts := test.DefaultTestOptions
	opts.Cluster.Compression.Mode = server.CompressionOff
	opts.Cluster.PoolSize = -1
	opts.Debug = true
	opts.LeafNode.Compression.Mode = server.CompressionOff
	opts.NoLog = false
	opts.Port = -1
	opts.Trace = true
	opts.TraceVerbose = true

	s, err := server.NewServer(&opts)
	if err != nil {
		t.Fatal("failed to contruct NATS server")
	}
	s.ConfigureLogger()
	go s.Start()
	if !s.ReadyForConnections(10 * time.Second) {
		t.Fatal("failed to start NATS Server")
	}
	return s
}
