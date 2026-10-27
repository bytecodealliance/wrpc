package wrpc

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
)

// maxUnclaimedElements is the maximum number of elements, i.e. positions matched by a wildcard,
// which data is received for before they are indexed
const maxUnclaimedElements = 64

func indexPath(path ...uint32) string {
	p := make([]byte, len(path)*4)
	for i, v := range path {
		l := i * 4
		binary.LittleEndian.PutUint32(p[l:l+4], v)
	}
	return string(p)
}

func concretePath(path SubscribePath) (string, bool) {
	p := make([]byte, len(path)*4)
	for i, v := range path {
		if v == nil {
			return "", false
		}
		l := i * 4
		binary.LittleEndian.PutUint32(p[l:l+4], *v)
	}
	return string(p), true
}

func decodePath(p string) []uint32 {
	path := make([]uint32, len(p)/4)
	for i := range path {
		l := i * 4
		path[i] = binary.LittleEndian.Uint32([]byte(p[l : l+4]))
	}
	return path
}

// subscribePattern is a declared path or a prefix of one
type subscribePattern struct {
	declared bool
	// wildcard is the pattern of the positions nested in the path, if they are matched by a wildcard
	wildcard *subscribePattern
	nested   map[uint32]*subscribePattern
}

func (p *subscribePattern) insert(path SubscribePath) {
	for _, i := range path {
		if i == nil {
			if p.wildcard == nil {
				p.wildcard = &subscribePattern{}
			}
			p = p.wildcard
			continue
		}
		if p.nested == nil {
			p.nested = make(map[uint32]*subscribePattern)
		}
		n, ok := p.nested[*i]
		if !ok {
			n = &subscribePattern{}
			p.nested[*i] = n
		}
		p = n
	}
	p.declared = true
}

// resolve returns the pattern matching `path` and the positions of the elements in it
func (p *subscribePattern) resolve(path []uint32) (*subscribePattern, []int) {
	var elements []int
	for k, i := range path {
		if p.wildcard != nil {
			elements = append(elements, k)
			p = p.wildcard
		} else if p = p.nested[i]; p == nil {
			return nil, nil
		}
	}
	return p, elements
}

type frameElement struct {
	readers int
	parent  *frameElement
	// paths are the subscriptions, which this is the innermost element containing
	paths []string
	// containers are the paths containing elements, which this is the innermost element containing
	containers []string
	// elements are the elements, which this is the innermost element containing
	elements map[string]struct{}
}

// frameSubscriptions are the subscriptions of the declared paths and the root.
//
// Elements, i.e. positions matched by a wildcard, are subscribed to once indexed or once data
// is received for them, and removed once all readers within them are dropped. Indexing an
// element releases the preceding ones, which were not indexed. At most [maxUnclaimedElements]
// elements are subscribed to before being indexed, after which ingress waits for them to be indexed.
type frameSubscriptions struct {
	ctx      context.Context
	readers  *atomic.Int64
	mu       sync.Mutex
	cond     *sync.Cond
	patterns subscribePattern
	nest     map[string]chan []byte
	closed   map[string]struct{}
	dropped  map[string]struct{}
	elements map[string]*frameElement
	// unclaimed are the elements subscribed to before being indexed
	unclaimed map[string]struct{}
	// next is the lowest index of an element, which is not released, by the path containing it
	next map[string]uint64
	done bool
}

// owner returns the innermost element containing position `k` of `path`, if any
func (s *frameSubscriptions) owner(path []uint32, positions []int, k int) *frameElement {
	var el *frameElement
	for _, j := range positions {
		if j >= k {
			break
		}
		el = s.elements[indexPath(path[:j+1]...)]
	}
	return el
}

// element returns the element at position `k` of `path`, subscribing to it if it is not yet
func (s *frameSubscriptions) element(path []uint32, positions []int, k int) (string, *frameElement) {
	e := indexPath(path[:k+1]...)
	el, ok := s.elements[e]
	if !ok {
		el = &frameElement{
			parent:   s.owner(path, positions, k),
			elements: make(map[string]struct{}),
		}
		if el.parent != nil {
			el.parent.elements[e] = struct{}{}
		}
		s.elements[e] = el
	}
	return e, el
}

// subscribe returns the channel of `path`, subscribing to it if it is not yet
func (s *frameSubscriptions) subscribe(path []uint32, positions []int) chan []byte {
	p := indexPath(path...)
	ch, ok := s.nest[p]
	if !ok {
		ch = make(chan []byte, 64)
		if s.done {
			close(ch)
		}
		s.nest[p] = ch
		if el := s.owner(path, positions, len(path)); el != nil {
			el.paths = append(el.paths, p)
		}
	}
	return ch
}

// route returns the channel of `path` and whether it is within a released element, waiting
// while [maxUnclaimedElements] elements are subscribed to before being indexed
func (s *frameSubscriptions) route(path []uint32) (chan []byte, bool, error) {
	if ch, ok := s.nest[indexPath(path...)]; ok {
		return ch, false, nil
	}
	for {
		pattern, positions := s.patterns.resolve(path)
		if pattern == nil || !pattern.declared {
			return nil, false, fmt.Errorf("received a frame for unknown path `%v`", path)
		}
		missing := 0
		for _, k := range positions {
			if _, ok := s.elements[indexPath(path[:k+1]...)]; ok {
				continue
			}
			if uint64(path[k]) < s.next[indexPath(path[:k]...)] {
				return nil, true, nil
			}
			missing++
		}
		if missing == 0 || len(s.unclaimed)+missing <= maxUnclaimedElements {
			for _, k := range positions {
				if _, ok := s.elements[indexPath(path[:k+1]...)]; !ok {
					e, _ := s.element(path, positions, k)
					s.unclaimed[e] = struct{}{}
				}
			}
			return s.subscribe(path, positions), false, nil
		}
		if err := s.ctx.Err(); err != nil {
			return nil, false, err
		}
		if s.readers.Load() == 0 {
			return nil, false, fmt.Errorf("received data for a closed reader on path `%v`", path)
		}
		s.cond.Wait()
	}
}

// claim subscribes to the elements in `path`, releasing the preceding ones, which were not indexed,
// and returns the channel of `path` and the elements in it
func (s *frameSubscriptions) claim(path []uint32) (chan []byte, []string, error) {
	pattern, positions := s.patterns.resolve(path)
	if pattern == nil || !pattern.declared {
		return nil, nil, errors.New("unknown subscription")
	}
	for _, k := range positions {
		if _, ok := s.elements[indexPath(path[:k+1]...)]; !ok && uint64(path[k]) < s.next[indexPath(path[:k]...)] {
			return nil, nil, errors.New("released subscription")
		}
	}
	elements := make([]string, len(positions))
	for j, k := range positions {
		c := indexPath(path[:k]...)
		next, ok := s.next[c]
		for p := range s.unclaimed {
			if len(p) != len(c)+4 || !strings.HasPrefix(p, c) {
				continue
			}
			if i := uint64(binary.LittleEndian.Uint32([]byte(p[len(c):]))); i >= next && i < uint64(path[k]) {
				s.remove(p)
			}
		}
		e, el := s.element(path, positions, k)
		el.readers++
		if _, ok := s.unclaimed[e]; ok {
			delete(s.unclaimed, e)
			s.cond.Broadcast()
		}
		if !ok {
			if owner := s.owner(path, positions, k); owner != nil {
				owner.containers = append(owner.containers, c)
			}
		}
		s.next[c] = max(next, uint64(path[k])+1)
		elements[j] = e
	}
	return s.subscribe(path, positions), elements, nil
}

// remove removes the element at `e` and everything nested in it
func (s *frameSubscriptions) remove(e string) {
	el, ok := s.elements[e]
	if !ok {
		return
	}
	delete(s.elements, e)
	delete(s.unclaimed, e)
	if el.parent != nil {
		delete(el.parent.elements, e)
	}
	for _, p := range el.paths {
		delete(s.nest, p)
		delete(s.closed, p)
		delete(s.dropped, p)
	}
	for _, c := range el.containers {
		delete(s.next, c)
	}
	for n := range el.elements {
		s.remove(n)
	}
	s.cond.Broadcast()
}

type FrameStreamReader struct {
	ctx      context.Context
	ch       <-chan []byte
	readers  *atomic.Int64
	subs     *frameSubscriptions
	elements []string
	err      *atomic.Pointer[error]
	path     string
	buf      []byte
	gone     atomic.Bool
}

func newFrameStreamReader(r *FrameStreamReader) *FrameStreamReader {
	runtime.SetFinalizer(r, func(r *FrameStreamReader) {
		slog.DebugContext(r.ctx, "closing unused stream reader")
		r.drop()
	})
	return r
}

func NewFrameStreamReader(ctx context.Context, r ByteReadCloser, paths ...SubscribePath) *FrameStreamReader {
	ch := make(chan []byte, 64)

	var readers atomic.Int64
	readers.Add(1)

	subs := &frameSubscriptions{
		ctx:       ctx,
		readers:   &readers,
		nest:      make(map[string]chan []byte, len(paths)+1),
		closed:    make(map[string]struct{}),
		dropped:   make(map[string]struct{}),
		elements:  make(map[string]*frameElement),
		unclaimed: make(map[string]struct{}),
		next:      make(map[string]uint64),
	}
	subs.cond = sync.NewCond(&subs.mu)
	subs.nest[""] = ch
	for _, path := range paths {
		subs.patterns.insert(path)
		if p, ok := concretePath(path); ok {
			subs.nest[p] = make(chan []byte, 64)
		}
	}
	stop := context.AfterFunc(ctx, func() {
		subs.mu.Lock()
		subs.cond.Broadcast()
		subs.mu.Unlock()
	})

	var ingressErr atomic.Pointer[error]
	go func() {
		defer func() {
			stop()
			subs.mu.Lock()
			subs.done = true
			for p, ch := range subs.nest {
				if _, ok := subs.closed[p]; !ok {
					close(ch)
				}
			}
			subs.mu.Unlock()

			if err := r.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close frame stream reader", "err", err)
			}
		}()

		handle := func(frame Frame) error {
			p := indexPath(frame.Path...)
			subs.mu.Lock()
			defer subs.mu.Unlock()
			ch, released, err := subs.route(frame.Path)
			if err != nil {
				return err
			}
			if released {
				if len(frame.Data) > 0 {
					return fmt.Errorf("received data for a released element on path `%v`", frame.Path)
				}
				return nil
			}
			if _, ok := subs.closed[p]; ok {
				return fmt.Errorf("received a frame for closed path `%v`", frame.Path)
			}
			if _, ok := subs.dropped[p]; ok && len(frame.Data) > 0 {
				return fmt.Errorf("received data for a closed reader on path `%v`", frame.Path)
			}
			if len(frame.Data) == 0 {
				slog.DebugContext(ctx, "received empty frame, closing path", "path", frame.Path)
				subs.closed[p] = struct{}{}
				close(ch)
				return nil
			}
			slog.DebugContext(ctx, "received data frame, sending frame data",
				"frame", frame,
			)
			select {
			case <-ctx.Done():
				return ctx.Err()
			case ch <- frame.Data:
				slog.DebugContext(ctx, "sent received frame data",
					"frame", frame,
				)
				return nil
			default:
				return fmt.Errorf("reader too slow, dropped a frame for path `%v`", frame.Path)
			}
		}
		for readers.Load() > 0 {
			slog.DebugContext(ctx, "reading frame")
			frame, err := ReadFrame(r)
			if err == io.EOF {
				break
			}
			if err == nil {
				slog.DebugContext(ctx, "read frame",
					"frame", frame,
				)
				err = handle(frame)
			}
			if err != nil {
				slog.ErrorContext(ctx, "failed to process frame", "err", err)
				ingressErr.Store(&err)
				break
			}
		}
	}()
	return newFrameStreamReader(&FrameStreamReader{
		ctx:     ctx,
		ch:      ch,
		readers: &readers,
		subs:    subs,
		err:     &ingressErr,
	})
}

func (r *FrameStreamReader) Read(p []byte) (int, error) {
	slog.DebugContext(r.ctx, "reading bytes",
		"path", r.path,
		"requested", len(p),
	)
	if len(r.buf) > 0 {
		n := copy(p, r.buf)
		slog.DebugContext(r.ctx, "copied bytes from buffer",
			"path", r.path,
			"requested", len(p),
			"buffered", len(r.buf),
			"copied", n,
		)
		r.buf = r.buf[n:]
		return n, nil
	}
	slog.DebugContext(r.ctx, "waiting for next frame",
		"path", r.path,
	)
	select {
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	case buf, ok := <-r.ch:
		if !ok {
			return 0, r.closedErr()
		}
		slog.DebugContext(r.ctx, "received data frame",
			"path", r.path,
			"buf", buf,
		)
		n := copy(p, buf)
		if n < len(buf) {
			r.buf = buf[n:]
		}
		return n, nil
	}
}

func (r *FrameStreamReader) ReadByte() (byte, error) {
	slog.DebugContext(r.ctx, "reading byte", "path", r.path)
	if len(r.buf) > 0 {
		b := r.buf[0]
		r.buf = r.buf[1:]
		slog.DebugContext(r.ctx, "copied byte from buffer",
			"path", r.path,
			"buffered", len(r.buf),
		)
		return b, nil
	}
	slog.DebugContext(r.ctx, "waiting for next frame", "path", r.path)
	select {
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	case buf, ok := <-r.ch:
		if !ok {
			return 0, r.closedErr()
		}
		slog.DebugContext(r.ctx, "received data frame", "path", r.path)
		if len(buf) > 1 {
			r.buf = buf[1:]
		}
		return buf[0], nil
	}
}

func (r *FrameStreamReader) Buffered() int {
	return len(r.buf)
}

func (r *FrameStreamReader) closedErr() error {
	r.subs.mu.Lock()
	_, closed := r.subs.closed[r.path]
	r.subs.mu.Unlock()
	if err := r.err.Load(); err != nil && !closed {
		return *err
	}
	return io.EOF
}

func (r *FrameStreamReader) Index(path ...uint32) (IndexReadCloser, error) {
	full := append(decodePath(r.path), path...)
	r.subs.mu.Lock()
	defer r.subs.mu.Unlock()
	ch, elements, err := r.subs.claim(full)
	if err != nil {
		return nil, err
	}
	readers := r.readers.Add(1)
	slog.DebugContext(r.ctx, "indexing reader",
		"readers", readers,
		"path", full,
	)
	return newFrameStreamReader(&FrameStreamReader{
		ctx:      r.ctx,
		ch:       ch,
		readers:  r.readers,
		subs:     r.subs,
		elements: elements,
		err:      r.err,
		path:     indexPath(full...),
	}), nil
}

func (r *FrameStreamReader) drop() {
	if r.gone.Swap(true) {
		return
	}
	readers := r.readers.Add(-1)
	slog.DebugContext(r.ctx, "dropped frame stream reader",
		"readers", readers,
		"path", r.path,
	)

	r.subs.mu.Lock()
	defer r.subs.mu.Unlock()

	r.subs.dropped[r.path] = struct{}{}
	r.subs.cond.Broadcast()
	for _, e := range r.elements {
		el, ok := r.subs.elements[e]
		if !ok {
			return
		}
		el.readers--
		if el.readers == 0 {
			r.subs.remove(e)
			return
		}
	}
}

func (r *FrameStreamReader) Close() error {
	defer runtime.SetFinalizer(r, nil)
	r.drop()
	return nil
}
