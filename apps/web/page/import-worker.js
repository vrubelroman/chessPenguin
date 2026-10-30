// The page's reader: a second instance of the program that reads the databases the user
// opens, so that the page does not stop while a large one is read
// (apps/desktop/src/platform/web/import.rs). One file at a time, in order.
import init, { read_database } from './chesspenguin_web.js';

const ready = init();

self.onmessage = async (e) => {
  await ready;
  const { id, name, bytes, count } = e.data;
  const answer = read_database(name, bytes, count);
  answer.id = id;
  self.postMessage(answer, answer.text ? [answer.text.buffer] : []);
};
