import { useState, useEffect } from "react";
import "./App.css";

type Theme = "light" | "dark" | "system";

function App() {
  const [text, setText] = useState("");
  const [theme, setTheme] = useState<Theme>("system");

  useEffect(() => {
    const root = document.documentElement;

    // Function to apply the theme
    const applyTheme = () => {
      if (theme === "system") {
        const systemTheme = window.matchMedia("(prefers-color-scheme: dark)").matches
          ? "dark"
          : "light";
        root.setAttribute("data-theme", systemTheme);
      } else {
        root.setAttribute("data-theme", theme);
      }
    };

    // Apply theme immediately
    applyTheme();

    // Listen for system theme changes only when in system mode
    if (theme === "system") {
      const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
      const handleChange = () => applyTheme();
      mediaQuery.addEventListener("change", handleChange);
      return () => mediaQuery.removeEventListener("change", handleChange);
    }
  }, [theme]);

  function handleRecord() {
    setText((prev) => prev + "hello world");
  }

  function handleCopy() {
    navigator.clipboard.writeText(text);
  }

  function cycleTheme() {
    setTheme((prev) => {
      if (prev === "system") return "light";
      if (prev === "light") return "dark";
      return "system";
    });
  }

  function getThemeLabel() {
    if (theme === "system") return "System";
    if (theme === "light") return "Light";
    return "Dark";
  }

  return (
    <main className="container">
      <button className="theme-toggle" onClick={cycleTheme}>
        {getThemeLabel()}
      </button>
      <button className="record-btn" onClick={handleRecord}>
        Record
      </button>
      <div className="text-box-container">
        <div className="text-box" spellCheck={false}>
          {text}
        </div>
        {text && (
          <button className="copy-btn" onClick={handleCopy}>
            Copy
          </button>
        )}
      </div>
    </main>
  );
}

export default App;
