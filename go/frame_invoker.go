package wrpc

import (
	"bufio"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"unicode/utf8"
)

// InvokeFramed invokes a function `name` within an instance `instance`, both of which must be valid UTF-8.
// `paths` define the async result paths to subscribe on.
// On success, `InvokeFramed` returns two handles used for writing and reading encoded parameters and results respectively.
// The encoded parameters are written to the root of the returned writer.
func InvokeFramed(ctx context.Context, w io.WriteCloser, r io.ReadCloser, instance string, name string, paths ...SubscribePath) (IndexWriteCloser, IndexReadCloser, error) {
	if len(instance) > math.MaxUint32 {
		return nil, nil, errors.New("instance length overflows uint32")
	}
	if len(name) > math.MaxUint32 {
		return nil, nil, errors.New("name length overflows uint32")
	}
	if !utf8.ValidString(instance) {
		return nil, nil, errors.New("instance is not valid UTF-8")
	}
	if !utf8.ValidString(name) {
		return nil, nil, errors.New("name is not valid UTF-8")
	}
	payload := make(
		[]byte,
		1+ // version
			binary.MaxVarintLen32+len(instance)+
			binary.MaxVarintLen32+len(name),
	)

	payload[0] = frameVersion
	i := 1

	i = i + PutUint32(payload[i:], uint32(len(instance)))
	i = i + copy(payload[i:], instance)

	i = i + PutUint32(payload[i:], uint32(len(name)))
	i = i + copy(payload[i:], name)

	_, err := w.Write(payload[:i])
	if err != nil {
		return nil, nil, fmt.Errorf("failed to write invocation header: %w", err)
	}
	return NewFrameStreamWriter(ctx, w), NewFrameStreamReader(ctx, BufReadCloser{
		Reader: bufio.NewReader(r),
		Closer: r,
	}, paths...), nil
}
