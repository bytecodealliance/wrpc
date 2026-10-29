//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world sync-client --out-dir bindings/sync_client --package wrpc.io/tests/go/bindings/sync_client ../wit

package integration_test

import (
	"context"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/sync_client/foo"
	"wrpc.io/tests/go/bindings/sync_client/wrpc_test/integration/sync"
	"wrpc.io/tests/go/bindings/sync_server"
	"wrpc.io/tests/go/internal"
)

func serve(t *testing.T, f func(wrpc.Server) (func() error, error)) (context.Context, wrpc.Invoker) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	t.Cleanup(cancel)
	srv, client := internal.RunTCP(t, ctx)
	stop, err := f(srv)
	require.NoError(t, err, "failed to serve world")
	t.Cleanup(func() { assert.NoError(t, stop(), "failed to stop serving world") })
	internal.Accept(ctx, srv)
	return ctx, client
}

func TestSync(t *testing.T) {
	var h integration.SyncHandler
	ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
		return sync_server.Serve(s, h, h)
	})

	n, err := foo.F(ctx, client, "f")
	require.NoError(t, err)
	assert.Equal(t, uint32(42), n)

	require.NoError(t, foo.Foo(ctx, client, "foo"))

	for ok, want := range map[bool]*wrpc.Result[bool, string]{
		true:  wrpc.Ok[string](true),
		false: wrpc.Err[bool]("test"),
	} {
		v, err := sync.Fallible(ctx, client, ok)
		require.NoError(t, err)
		assert.Equal(t, want, v)
	}

	v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, err := sync.Numbers(ctx, client)
	require.NoError(t, err)
	assert.Equal(t,
		[]any{uint8(1), uint16(2), uint32(3), uint64(4), int8(5), int16(6), int32(7), int64(8), float32(9), float64(10)},
		[]any{v0, v1, v2, v3, v4, v5, v6, v7, v8, v9},
	)

	flags, err := sync.WithFlags(ctx, client, true, false, true)
	require.NoError(t, err)
	assert.Equal(t, &sync.Abc{A: true, C: true}, flags)

	rec := func(foo string) *sync.Rec {
		return &sync.Rec{Nested: &sync.RecNested{Foo: foo}}
	}

	opt, err := sync.WithVariantOption(ctx, client, true)
	require.NoError(t, err)
	assert.Equal(t, sync.NewVarVar(rec("bar")), opt)

	opt, err = sync.WithVariantOption(ctx, client, false)
	require.NoError(t, err)
	assert.Nil(t, opt)

	vars, err := sync.WithVariantList(ctx, client)
	require.NoError(t, err)
	empty := sync.NewVarEmpty()
	assert.Equal(t, []*sync.Var{empty, sync.NewVarVar(rec("foo")), empty, empty, empty, sync.NewVarVar(rec("bar"))}, vars)

	r, err := sync.WithRecord(ctx, client)
	require.NoError(t, err)
	assert.Equal(t, rec("foo"), r)

	rs, err := sync.WithRecordList(ctx, client, 3)
	require.NoError(t, err)
	assert.Equal(t, []*sync.Rec{rec("0"), rec("1"), rec("2")}, rs)
}
