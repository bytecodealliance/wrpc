package wrpcnats

import (
	"context"
	"log/slog"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats-server/v2/test"
	"github.com/nats-io/nats.go"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
)

func runNATSServer(t *testing.T) *server.Server {
	opts := test.DefaultTestOptions
	opts.Port = -1
	opts.Debug = false
	opts.NoLog = true

	s, err := server.NewServer(&opts)
	require.NoError(t, err, "failed to construct NATS server")

	go s.Start()
	require.True(t, s.ReadyForConnections(10*time.Second), "failed to start NATS server")

	t.Cleanup(func() {
		s.Shutdown()
		s.WaitForShutdown()
	})

	return s
}

func setupNATSClient(t *testing.T, srv *server.Server) *Client {
	nc, err := nats.Connect(srv.ClientURL())
	require.NoError(t, err, "failed to connect to NATS")

	t.Cleanup(func() {
		nc.Drain()
		nc.Close()
	})

	return NewClient(nc, WithPrefix("test"))
}

func TestConcurrentRequestHandling(t *testing.T) {
	srv := runNATSServer(t)
	client := setupNATSClient(t, srv)

	// Track concurrent execution
	var activeRequests atomic.Int32
	var maxConcurrent atomic.Int32
	var requestsCompleted atomic.Int32

	// Handler that simulates slow processing and tracks concurrency
	handler := func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		current := activeRequests.Add(1)
		defer activeRequests.Add(-1)
		defer requestsCompleted.Add(1)

		// Update max concurrent if necessary
		for {
			max := maxConcurrent.Load()
			if current <= max || maxConcurrent.CompareAndSwap(max, current) {
				break
			}
		}

		// Simulate slow processing
		time.Sleep(100 * time.Millisecond)

		// Echo back a simple response
		_, err := w.Write([]byte("response"))
		if err != nil {
			slog.Error("failed to write response", "err", err)
		}
		w.Close()
	}

	// Start serving
	stop, err := client.Serve("test-instance", "slow-handler", handler)
	require.NoError(t, err, "failed to start serving")
	defer stop()

	// Give the server a moment to be ready
	time.Sleep(10 * time.Millisecond)

	// Send multiple concurrent requests
	numRequests := 5
	var wg sync.WaitGroup
	wg.Add(numRequests)

	start := time.Now()

	for i := 0; i < numRequests; i++ {
		go func(requestID int) {
			defer wg.Done()

			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()

			// Invoke the slow handler
			w, r, err := client.Invoke(ctx, "test-instance", "slow-handler", []byte("request"))
			if err != nil {
				t.Errorf("request %d failed to invoke: %v", requestID, err)
				return
			}

			// Close the writer immediately since we're not sending more data
			w.Close()

			// Read the response
			buf := make([]byte, 1024)
			n, err := r.Read(buf)
			if err != nil {
				t.Errorf("request %d failed to read response: %v", requestID, err)
				return
			}

			response := string(buf[:n])
			if response != "response" {
				t.Errorf("request %d got unexpected response: %s", requestID, response)
			}

			r.Close()
		}(i)
	}

	// Wait for all requests to complete
	wg.Wait()
	duration := time.Since(start)

	// Verify results
	assert.Equal(t, int32(numRequests), requestsCompleted.Load(), "not all requests completed")
	assert.GreaterOrEqual(t, maxConcurrent.Load(), int32(2), "should have at least 2 concurrent requests")

	// If requests were processed sequentially, it would take at least numRequests * 100ms
	// With concurrency, it should be much faster
	sequentialTime := time.Duration(numRequests) * 100 * time.Millisecond
	assert.Less(t, duration, sequentialTime*8/10, "requests should be processed concurrently, not sequentially")

	t.Logf("Processed %d requests in %v (max concurrent: %d)",
		numRequests, duration, maxConcurrent.Load())
}

func TestPanicRecovery(t *testing.T) {
	srv := runNATSServer(t)
	client := setupNATSClient(t, srv)

	// Handler that panics
	panicHandler := func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		panic("test panic")
	}

	// Start serving
	stop, err := client.Serve("test-instance", "panic-handler", panicHandler)
	require.NoError(t, err, "failed to start serving")
	defer stop()

	// Give the server a moment to be ready
	time.Sleep(10 * time.Millisecond)

	// Send a request that will cause a panic
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	w, r, err := client.Invoke(ctx, "test-instance", "panic-handler", []byte("request"))
	require.NoError(t, err, "failed to invoke panic handler")

	w.Close()
	r.Close()

	// The server should still be responsive after the panic
	// Test with a normal handler
	normalHandler := func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		w.Write([]byte("ok"))
		w.Close()
	}

	stop2, err := client.Serve("test-instance", "normal-handler", normalHandler)
	require.NoError(t, err, "failed to start serving normal handler after panic")
	defer stop2()

	time.Sleep(10 * time.Millisecond)

	// This should work normally
	w2, r2, err := client.Invoke(ctx, "test-instance", "normal-handler", []byte("request"))
	require.NoError(t, err, "failed to invoke normal handler after panic recovery")

	w2.Close()

	buf := make([]byte, 1024)
	n, err := r2.Read(buf)
	require.NoError(t, err, "failed to read response from normal handler")
	assert.Equal(t, "ok", string(buf[:n]))

	r2.Close()
}
