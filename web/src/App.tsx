import { Route, Router } from "@solidjs/router";
import { Shell } from "./components/Shell";
import Home from "./routes/Home";
import UassetPage from "./routes/Uasset";
import UtracePage from "./routes/Utrace";

export default function App() {
  return (
    <Router root={Shell}>
      <Route path="/" component={Home} />
      <Route path="/uasset" component={UassetPage} />
      <Route path="/utrace" component={UtracePage} />
    </Router>
  );
}
