import { Counter, formatCount } from './counter'

const counter = new Counter(1)
const app = document.querySelector<HTMLDivElement>('#app')!

function render() {
  app.textContent = formatCount(counter.count, 'Taps')
}

app.addEventListener('click', () => {
  counter.increment()
  render()
})

render()
