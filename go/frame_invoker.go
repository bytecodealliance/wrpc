package wrpc

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
)

// InvokeFramed invokes a function `name` within an instance `instance`.
// Initial, encoded payload must be specified in `buf`.
// `paths` define the async result paths to subscribe on.
// On success, `InvokeFramed` returns two handles used for writing and reading encoded parameters and results respectively.
// NOTE: if the returned handle is used for writing, `b` must be non-empty.
func InvokeFramed(ctx context.Context, w io.WriteCloser, r io.ReadCloser, instance string, name string, buf []byte, paths ...SubscribePath) (IndexWriteCloser, IndexReadCloser, error) {
	if len(instance) > math.MaxUint32 {
		return nil, nil, errors.New("instance length overflows uint32")
	}
	if len(name) > math.MaxUint32 {
		return nil, nil, errors.New("name length overflows uint32")
	}
	if len(buf) > math.MaxUint32 {
		return nil, nil, errors.New("buffer length overflows uint32")
	}
	// TODO: Count the exact amount of bytes required
	payload := make(
		[]byte,
		1+ // version
			binary.MaxVarintLen32+len(instance)+
			binary.MaxVarintLen32+len(name)+
			1+ // empty path length
			binary.MaxVarintLen32+len(buf),
	)

	i := 1 // version == 0

	i = i + PutUint32(payload[i:], uint32(len(instance)))
	i = i + copy(payload[i:], instance)

	i = i + PutUint32(payload[i:], uint32(len(name)))
	i = i + copy(payload[i:], name)

	i = i + 1 // path == []

	i = i + binary.PutUvarint(payload[i:], uint64(len(buf)))
	i = i + copy(payload[i:], buf)

	_, err := w.Write(payload[:i])
	if err != nil {
		return nil, nil, fmt.Errorf("failed to write initial frame: %w", err)
	}
	return NewFrameStreamWriter(ctx, w), NewFrameStreamReader(ctx, r, paths...), nil
}
