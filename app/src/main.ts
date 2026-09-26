import { mount } from "svelte";
import "@fontsource-variable/fraunces/full.css";
import "@fontsource-variable/karla/wght.css";
import "./app.css";
import App from "./App.svelte";

const app = mount(App, { target: document.getElementById("app")! });

export default app;
