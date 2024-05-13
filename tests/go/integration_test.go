package integration_test

import (
	"context"
	"io"
	"log/slog"
	"os"
	"reflect"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats-server/v2/test"
	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	integration "github.com/wrpc/wrpc/tests/go"
	"github.com/wrpc/wrpc/tests/go/bindings/async_client/wrpc_test/integration/async"
	"github.com/wrpc/wrpc/tests/go/bindings/async_server"
	"github.com/wrpc/wrpc/tests/go/bindings/sync_client/foo"
	"github.com/wrpc/wrpc/tests/go/bindings/sync_client/wrpc_test/integration/sync"
	"github.com/wrpc/wrpc/tests/go/bindings/sync_server"
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

func runNats(t *testing.T) *server.Server {
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

func TestSync(t *testing.T) {
	natsSrv := runNats(t)
	nc, err := nats.Connect(natsSrv.ClientURL())
	if err != nil {
		t.Errorf("failed to connect to NATS.io: %s", err)
		return
	}
	defer nc.Close()
	defer func() {
		if err := nc.Drain(); err != nil {
			t.Errorf("failed to drain NATS.io connection: %s", err)
			return
		}
	}()
	client := wrpcnats.NewClient(nc, "go")

	stop, err := sync_server.Serve(client, integration.SyncHandler{})
	if err != nil {
		t.Errorf("failed to serve `sync-server` world: %s", err)
		return
	}

	var cancel func()
	ctx := context.Background()
	dl, ok := t.Deadline()
	if ok {
		ctx, cancel = context.WithDeadline(ctx, dl)
	} else {
		ctx, cancel = context.WithTimeout(ctx, time.Minute)
	}
	defer cancel()

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/sync-client.foo.f`")
		v, shutdown, err := foo.F(ctx, client, "f")
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync-client.foo.f`: %s", err)
			return
		}
		if v != 42 {
			t.Errorf("expected: 42, got: %d", v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/sync-client.foo.foo`")
		shutdown, err := foo.Foo(ctx, client, "foo")
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync-client.foo.foo`: %s", err)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/sync.fallible`")
		v, shutdown, err := sync.Fallible(ctx, client, true)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.fallible`: %s", err)
			return
		}
		expected := wrpc.Ok[string](true)
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %#v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/sync.fallible`")
		v, shutdown, err := sync.Fallible(ctx, client, false)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.fallible`: %s", err)
			return
		}
		expected := wrpc.Err[bool]("test")
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %#v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/sync.numbers`")
		v, shutdown, err := sync.Numbers(ctx, client)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.numbers`: %s", err)
			return
		}
		expected := &wrpc.Tuple10[uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64]{V0: 1, V1: 2, V2: 3, V3: 4, V4: 5, V5: 6, V6: 7, V7: 8, V8: 9, V9: 10}
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/sync.with-flags`")
		v, shutdown, err := sync.WithFlags(ctx, client, true, false, true)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.with-flags`: %s", err)
			return
		}
		expected := &sync.Abc{A: true, B: false, C: true}
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		v, shutdown, err := sync.WithVariantOption(ctx, client, true)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.with-variant-option`: %s", err)
			return
		}
		expected := sync.NewVar_Var(&sync.Rec{
			Nested: &sync.RecNested{
				Foo: "bar",
			},
		})
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		v, shutdown, err := sync.WithVariantOption(ctx, client, false)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.with-variant-option`: %s", err)
			return
		}
		var expected *sync.Var
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		v, shutdown, err := sync.WithRecord(ctx, client)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.with-record`: %s", err)
			return
		}
		expected := &sync.Rec{
			Nested: &sync.RecNested{
				Foo: "foo",
			},
		}
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}
	{
		v, shutdown, err := sync.WithRecordList(ctx, client, 3)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/sync.with-record-list`: %s", err)
			return
		}
		expected := []*sync.Rec{
			{
				Nested: &sync.RecNested{
					Foo: "0",
				},
			},
			{
				Nested: &sync.RecNested{
					Foo: "1",
				},
			},
			{
				Nested: &sync.RecNested{
					Foo: "2",
				},
			},
		}
		if !reflect.DeepEqual(v, expected) {
			t.Errorf("expected: %v, got: %#v", expected, v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `sync-server` world: %s", err)
		return
	}
}

func TestAsync(t *testing.T) {
	natsSrv := runNats(t)
	nc, err := nats.Connect(natsSrv.ClientURL())
	if err != nil {
		t.Errorf("failed to connect to NATS.io: %s", err)
		return
	}
	defer nc.Close()
	defer func() {
		if err := nc.Drain(); err != nil {
			t.Errorf("failed to drain NATS.io connection: %s", err)
			return
		}
	}()
	client := wrpcnats.NewClient(nc, "go")

	stop, err := async_server.Serve(client, integration.AsyncHandler{})
	if err != nil {
		t.Errorf("failed to serve `async-server` world: %s", err)
		return
	}

	var cancel func()
	ctx := context.Background()
	dl, ok := t.Deadline()
	if ok {
		ctx, cancel = context.WithDeadline(ctx, dl)
	} else {
		ctx, cancel = context.WithTimeout(ctx, time.Minute)
	}
	defer cancel()

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.with-stream`")
		v, shutdown, err := async.WithStream(ctx, client)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/async.with-stream`: %s", err)
			return
		}
		b, err := io.ReadAll(v)
		if err != nil {
			t.Errorf("failed to read from stream: %s", err)
			return
		}
		if string(b) != "test" {
			t.Errorf("expected: `test`, got: %s", string(b))
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `sync-server` world: %s", err)
		return
	}
}
