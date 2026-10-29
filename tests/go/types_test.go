//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world types --out-dir bindings/types --package wrpc.io/tests/go/bindings/types ../wit

package integration_test

import (
	"bytes"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	"wrpc.io/tests/go/bindings/types/wrpc_test/integration/get_types"
)

type indexReader struct {
	*bytes.Buffer
}

func (r *indexReader) Index(path ...uint32) (wrpc.IndexReadCloser, error) {
	panic("not implemented")
}

func (r *indexReader) Buffered() int {
	return r.Len()
}

func TestFlags(t *testing.T) {
	flags := get_types.FeatureFlags{ShowA: true, ShowC: true}
	var b bytes.Buffer
	_, err := flags.WriteToIndex(&b)
	require.NoError(t, err)
	assert.Equal(t, []byte{0b101}, b.Bytes())

	var got get_types.FeatureFlags
	require.NoError(t, got.ReadFromIndex(&indexReader{&b}))
	assert.Equal(t, flags, got)

	got = get_types.FeatureFlags{}
	assert.EqualError(t, got.ReadFromIndex(&indexReader{bytes.NewBuffer([]byte{0x80})}), "bit not associated with any flag is set")
	assert.Zero(t, got)
}
