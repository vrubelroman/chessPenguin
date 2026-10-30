// The page's reader: a second instance of the program that reads the databases the user
// opens, so that the page does not stop while a large one is read
// (apps/desktop/src/platform/web/import.rs). One database at a time, in order: its files
// one by one (the user's are the browser's File objects, read here), then the reading.
import init, { reader_give, reader_read } from './chesspenguin_web.js';

const ready = init();

self.onmessage = async (e) => {
  await ready;
  const { id, names, parts, count } = e.data;
  let answer;
  try {
    for (let i = 0; i < names.length; i++) {
      const part = parts[i];
      const bytes = part instanceof Blob ? new Uint8Array(await part.arrayBuffer()) : part;
      parts[i] = null;
      reader_give(names[i], bytes);
    }
    answer = reader_read(count);
  } catch (err) {
    // The program stopped (out of memory, most likely): this instance is spent.
    self.postMessage({ id, crashed: String(err) });
    self.close();
    return;
  }
  answer.id = id;
  self.postMessage(answer, answer.text ? [answer.text.buffer] : []);
};
